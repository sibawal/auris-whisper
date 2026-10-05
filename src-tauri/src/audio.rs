//! Любой аудио- или видеофайл → 16 кГц, моно, f32 — то, что ест whisper.
//!
//! Контейнеры и кодеки разбирает symphonia (mp3, aac/m4a, mp4/mov, flac, wav,
//! aiff, caf, ogg/vorbis, mkv/webm). Opus — голосовые из мессенджеров — она не
//! декодирует, поэтому пакеты Opus отдаём libopus. Формат определяется по
//! содержимому, а не по расширению: переименованные голосовые тоже читаются.

use std::fs::File;
use std::path::Path;

use rubato::{FftFixedIn, Resampler};
use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::codecs::CodecParameters;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;

use crate::i18n::tr;

pub const SAMPLE_RATE: u32 = 16_000;

/// Декодирует файл целиком. Длинные записи читаются потоково:
/// в памяти копится только результат в 16 кГц (час — около 230 МБ).
///
/// Сначала symphonia (она же читает ogg/opus/webm/mkv, которых нет в системных
/// декодерах). Если не справилась — системный декодер или ffmpeg: видео с iPhone
/// (HEVC), AAC 5.1, AC-3, avi, ts, wmv и прочее.
pub fn decode_file(path: &Path) -> Result<Vec<f32>, String> {
    match decode_symphonia(path) {
        Ok(s) => Ok(s),
        Err(e) => match crate::native_decode::decode(path) {
            Some(s) => Ok(s),
            None => Err(if crate::native_decode::has_ffmpeg() {
                e
            } else {
                let install = if cfg!(target_os = "macos") {
                    "brew install ffmpeg"
                } else if cfg!(windows) {
                    "winget install ffmpeg"
                } else {
                    "sudo apt install ffmpeg"
                };
                let sep = if e.ends_with('.') { "" } else { "." };
                e + sep
                    + &tr(
                        &format!(" Установите ffmpeg ({install}) и перезапустите приложение — тогда откроется почти любой формат."),
                        &format!(" Install ffmpeg ({install}) and restart the app to open almost any format."),
                    )
            }),
        },
    }
}

fn decode_symphonia(path: &Path) -> Result<Vec<f32>, String> {
    let file = File::open(path).map_err(|e| tr("Не удалось открыть файл: ", "Could not open the file: ") + &e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(&ext.to_lowercase());
    }

    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| unsupported(&e))?;

    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| tr("В файле нет звуковой дорожки.", "The file has no audio track."))?
        .clone();
    let params = match &track.codec_params {
        Some(CodecParameters::Audio(p)) => p.clone(),
        _ => return Err(tr("В файле нет звуковой дорожки.", "The file has no audio track.")),
    };

    let mut sink = MonoSink::default();

    if params.codec == CODEC_ID_OPUS {
        let channels = params.channels.as_ref().map(|c| c.count()).unwrap_or(2).clamp(1, 2);
        let mut opus = OpusDecoder::new(channels)?;
        loop {
            match format.next_packet() {
                Ok(Some(packet)) if packet.track_id == track.id => {
                    if let Some(pcm) = opus.decode(&packet.data) {
                        sink.push_interleaved(&pcm, channels, 48_000)?;
                    }
                }
                Ok(Some(_)) => continue,
                Ok(None) => break,
                Err(SymError::ResetRequired) => break,
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(decode_err(&e)),
            }
        }
    } else {
        let mut decoder: Box<dyn AudioDecoder> = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .map_err(|e| unsupported(&e))?;

        let mut planes: Vec<Vec<f32>> = Vec::new();
        loop {
            let packet = match format.next_packet() {
                Ok(Some(p)) => p,
                Ok(None) => break,
                Err(SymError::ResetRequired) => break,
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(decode_err(&e)),
            };
            if packet.track_id != track.id {
                continue;
            }
            match decoder.decode(&packet) {
                Ok(buf) => {
                    let rate = buf.spec().rate();
                    if buf.frames() == 0 || rate == 0 {
                        continue;
                    }
                    buf.copy_to_vecs_planar::<f32>(&mut planes);
                    sink.push_planar(&planes, rate)?;
                }
                // Битый пакет посреди файла — пропускаем, как делают плееры.
                Err(SymError::DecodeError(_)) => continue,
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(decode_err(&e)),
            }
        }
    }

    let samples = sink.finish()?;
    if samples.len() < (SAMPLE_RATE / 10) as usize {
        return Err(tr("Аудио пустое или слишком короткое.", "The audio is empty or too short."));
    }
    Ok(samples)
}

fn unsupported(e: &SymError) -> String {
    match e {
        SymError::Unsupported(_) => tr(
            "Не удалось прочитать звук из этого файла: такой формат или кодек не поддерживается.",
            "Could not read audio from this file: the format or codec is not supported.",
        ),
        other => decode_err(other),
    }
}

fn decode_err(e: &SymError) -> String {
    tr("Не удалось декодировать аудио: ", "Could not decode the audio: ") + &e.to_string()
}

// MARK: - Сведение в моно и пересэмплирование

/// Собирает поток блоками любой частоты и отдаёт 16 кГц моно.
/// Частота может смениться посреди файла (бывает в склеенных mp3) —
/// тогда ресемплер пересоздаётся.
#[derive(Default)]
pub(crate) struct MonoSink {
    out: Vec<f32>,
    resampler: Option<(u32, StreamResampler)>,
    mono: Vec<f32>,
}

impl MonoSink {
    fn push_planar(&mut self, planes: &[Vec<f32>], rate: u32) -> Result<(), String> {
        let channels = planes.len().max(1);
        let frames = planes.first().map(|p| p.len()).unwrap_or(0);
        self.mono.clear();
        self.mono.resize(frames, 0.0);
        for plane in planes {
            for (m, s) in self.mono.iter_mut().zip(plane) {
                *m += *s;
            }
        }
        if channels > 1 {
            let k = 1.0 / channels as f32;
            self.mono.iter_mut().for_each(|m| *m *= k);
        }
        let mono = std::mem::take(&mut self.mono);
        let r = self.push_mono(&mono, rate);
        self.mono = mono;
        r
    }

    pub(crate) fn push_interleaved(&mut self, data: &[f32], channels: usize, rate: u32) -> Result<(), String> {
        let mono: Vec<f32> = if channels == 1 {
            data.to_vec()
        } else {
            data.chunks_exact(channels)
                .map(|f| f.iter().sum::<f32>() / channels as f32)
                .collect()
        };
        self.push_mono(&mono, rate)
    }

    fn push_mono(&mut self, mono: &[f32], rate: u32) -> Result<(), String> {
        let needs_new = match &self.resampler {
            Some((r, _)) => *r != rate,
            None => true,
        };
        if needs_new {
            if let Some((_, old)) = self.resampler.take() {
                self.out.extend(old.finish());
            }
            self.resampler = Some((rate, StreamResampler::new(rate)?));
        }
        let (_, rs) = self.resampler.as_mut().unwrap();
        rs.push(mono, &mut self.out);
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<Vec<f32>, String> {
        if let Some((_, rs)) = self.resampler.take() {
            self.out.extend(rs.finish());
        }
        Ok(self.out)
    }
}

/// Потоковый ресемплер любой частоты → 16 кГц (FFT, с антиалиасингом).
/// Один экземпляр на поток: состояние фильтра сохраняется между блоками,
/// поэтому на стыках нет щелчков.
pub struct StreamResampler {
    inner: Option<FftFixedIn<f32>>,
    pending: Vec<f32>,
    chunk: usize,
    /// Сколько отсчётов на выходе ещё отрезать: задержка фильтра.
    skip: usize,
    produced_from: u64,
    input_total: u64,
    rate: u32,
}

impl StreamResampler {
    pub fn new(input_rate: u32) -> Result<Self, String> {
        if input_rate == SAMPLE_RATE {
            return Ok(Self { inner: None, pending: Vec::new(), chunk: 0, skip: 0, produced_from: 0, input_total: 0, rate: input_rate });
        }
        let chunk = 1024;
        let rs = FftFixedIn::<f32>::new(input_rate as usize, SAMPLE_RATE as usize, chunk, 2, 1)
            .map_err(|e| format!("resampler: {e}"))?;
        let skip = rs.output_delay();
        Ok(Self { inner: Some(rs), pending: Vec::with_capacity(chunk * 2), chunk, skip, produced_from: 0, input_total: 0, rate: input_rate })
    }

    pub fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.input_total += input.len() as u64;
        let Some(rs) = self.inner.as_mut() else {
            out.extend_from_slice(input);
            return;
        };
        self.pending.extend_from_slice(input);
        let mut offset = 0;
        while self.pending.len() - offset >= self.chunk {
            let block = &self.pending[offset..offset + self.chunk];
            if let Ok(res) = rs.process(&[block], None) {
                Self::emit(&mut self.skip, &mut self.produced_from, &res[0], out);
            }
            offset += self.chunk;
        }
        self.pending.drain(..offset);
    }

    /// Дотянуть хвост и обрезать выход ровно по длине входа.
    pub fn finish(mut self) -> Vec<f32> {
        let mut out = Vec::new();
        let Some(mut rs) = self.inner.take() else { return out };
        if !self.pending.is_empty() {
            if let Ok(res) = rs.process_partial(Some(&[&self.pending[..]]), None) {
                Self::emit(&mut self.skip, &mut self.produced_from, &res[0], &mut out);
            }
        }
        // Ещё пара пустых проходов, чтобы вытолкнуть задержку фильтра.
        let expected = (self.input_total as f64 * SAMPLE_RATE as f64 / self.rate as f64).round() as u64;
        let mut guard = 0;
        while self.produced_from < expected && guard < 8 {
            match rs.process_partial::<&[f32]>(None, None) {
                Ok(res) => Self::emit(&mut self.skip, &mut self.produced_from, &res[0], &mut out),
                Err(_) => break,
            }
            guard += 1;
        }
        let overshoot = self.produced_from.saturating_sub(expected) as usize;
        out.truncate(out.len().saturating_sub(overshoot));
        out
    }

    fn emit(skip: &mut usize, produced: &mut u64, data: &[f32], out: &mut Vec<f32>) {
        let cut = (*skip).min(data.len());
        *skip -= cut;
        out.extend_from_slice(&data[cut..]);
        *produced += (data.len() - cut) as u64;
    }
}

// MARK: - Opus через libopus

struct OpusDecoder {
    st: *mut opusic_sys::OpusDecoder,
    channels: usize,
    buf: Vec<f32>,
}

// Декодер живёт и используется в одном потоке; указатель никуда не утекает.
unsafe impl Send for OpusDecoder {}

impl OpusDecoder {
    fn new(channels: usize) -> Result<Self, String> {
        let mut err = 0;
        let st = unsafe { opusic_sys::opus_decoder_create(48_000, channels as i32, &mut err) };
        if st.is_null() || err != opusic_sys::OPUS_OK {
            return Err(format!("libopus: decoder init failed ({err})"));
        }
        // 120 мс при 48 кГц — максимальная длина кадра Opus.
        Ok(Self { st, channels, buf: vec![0.0; 5760 * channels] })
    }

    fn decode(&mut self, packet: &[u8]) -> Option<Vec<f32>> {
        if packet.is_empty() {
            return None;
        }
        let frames = unsafe {
            opusic_sys::opus_decode_float(
                self.st,
                packet.as_ptr(),
                packet.len() as i32,
                self.buf.as_mut_ptr(),
                5760,
                0,
            )
        };
        if frames <= 0 {
            return None;
        }
        Some(self.buf[..frames as usize * self.channels].to_vec())
    }
}

impl Drop for OpusDecoder {
    fn drop(&mut self) {
        unsafe { opusic_sys::opus_decoder_destroy(self.st) }
    }
}

// MARK: - WAV

/// 16 кГц моно → WAV 16 бит: так сохраняем надиктованное рядом с текстом.
pub fn write_wav(samples: &[f32], path: &Path) -> std::io::Result<()> {
    use std::io::Write;
    let data_bytes = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_bytes as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // размер fmt-чанка
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // моно
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // байт/с
    out.extend_from_slice(&2u16.to_le_bytes()); // выравнивание блока
    out.extend_from_slice(&16u16.to_le_bytes()); // бит на отсчёт
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    File::create(path)?.write_all(&out)
}
