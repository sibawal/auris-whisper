//! Модели только для русской речи: GigaAM v3 (Сбер) и T-One (Т-Банк).
//! Считаются через sherpa-onnx на процессоре — и всё равно в разы быстрее whisper,
//! а русский понимают точнее.
//!
//! Обе модели рассчитаны на фразы по 10–20 секунд, поэтому запись сначала
//! режем детектором речи (тот же Silero, что и у whisper) на куски по паузам,
//! затем распознаём каждый кусок отдельно. Заодно получаем время фраз.

use std::ffi::{CStr, CString};
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use sherpa_onnx_sys as so;

use crate::audio::SAMPLE_RATE;
use crate::engine::{EngineError, Segment};
use crate::i18n::tr;
use crate::models::Backend;
use crate::vad::{speech_spans, threads, VadOptions};

/// Доля прогресса, которая приходится на поиск речи.
const VAD_SHARE: f64 = 0.1;

/// Самый длинный кусок, который отдаём модели.
const MAX_PIECE_SECS: f32 = 20.0;
/// Соседние фразы с паузой короче этой склеиваем — модели лучше с контекстом.
const JOIN_GAP_SECS: f64 = 0.8;
/// Сколько тишины оставлять по краям куска.
const PAD_SECS: f64 = 0.15;

enum Recognizer {
    Offline(*const so::OfflineRecognizer),
    Online(*const so::OnlineRecognizer),
}

pub struct RuModel {
    rec: Recognizer,
    backend: Backend,
}

// Распознаватель sherpa-onnx потокобезопасен для последовательных вызовов,
// а мы зовём его только из потока расшифровки под мьютексом движка.
unsafe impl Send for RuModel {}

impl Drop for RuModel {
    fn drop(&mut self) {
        unsafe {
            match self.rec {
                Recognizer::Offline(r) => so::SherpaOnnxDestroyOfflineRecognizer(r),
                Recognizer::Online(r) => so::SherpaOnnxDestroyOnlineRecognizer(r),
            }
        }
    }
}

fn cstr(p: &Path) -> Result<CString, EngineError> {
    p.to_str().and_then(|s| CString::new(s).ok()).ok_or_else(|| EngineError::Failed("bad model path".into()))
}


fn load_failed() -> EngineError {
    EngineError::Failed(tr(
        "Не удалось загрузить модель. Возможно, файлы повреждены — удалите её и скачайте заново.",
        "Could not load the model. The files may be damaged — delete it and download again.",
    ))
}

impl RuModel {
    pub fn load(backend: Backend, dir: &Path) -> Result<Self, EngineError> {
        let provider = CString::new("cpu").unwrap();
        let greedy = CString::new("greedy_search").unwrap();
        let tokens = cstr(&dir.join("tokens.txt"))?;
        let rec = match backend {
            Backend::Gigaam => {
                let encoder = cstr(&dir.join("encoder.int8.onnx"))?;
                let decoder = cstr(&dir.join("decoder.onnx"))?;
                let joiner = cstr(&dir.join("joiner.onnx"))?;
                let model_type = CString::new("nemo_transducer").unwrap();
                let mut cfg: so::OfflineRecognizerConfig = unsafe { std::mem::zeroed() };
                cfg.feat_config.sample_rate = SAMPLE_RATE as i32;
                cfg.feat_config.feature_dim = 64;
                cfg.model_config.transducer.encoder = encoder.as_ptr();
                cfg.model_config.transducer.decoder = decoder.as_ptr();
                cfg.model_config.transducer.joiner = joiner.as_ptr();
                cfg.model_config.tokens = tokens.as_ptr();
                cfg.model_config.num_threads = threads();
                cfg.model_config.provider = provider.as_ptr();
                cfg.model_config.model_type = model_type.as_ptr();
                cfg.decoding_method = greedy.as_ptr();
                cfg.max_active_paths = 4;
                let r = unsafe { so::SherpaOnnxCreateOfflineRecognizer(&cfg) };
                if r.is_null() {
                    return Err(load_failed());
                }
                Recognizer::Offline(r)
            }
            Backend::Tone => {
                let model = cstr(&dir.join("model.onnx"))?;
                let mut cfg: so::OnlineRecognizerConfig = unsafe { std::mem::zeroed() };
                // T-One обучена на телефонном звуке 8 кГц; sherpa-onnx сам
                // передискретизирует наши 16 кГц.
                cfg.feat_config.sample_rate = 8000;
                cfg.feat_config.feature_dim = 80;
                cfg.model_config.t_one_ctc.model = model.as_ptr();
                cfg.model_config.tokens = tokens.as_ptr();
                cfg.model_config.num_threads = threads();
                cfg.model_config.provider = provider.as_ptr();
                cfg.decoding_method = greedy.as_ptr();
                cfg.max_active_paths = 4;
                let r = unsafe { so::SherpaOnnxCreateOnlineRecognizer(&cfg) };
                if r.is_null() {
                    return Err(load_failed());
                }
                Recognizer::Online(r)
            }
            Backend::Whisper => return Err(EngineError::Failed("not a sherpa-onnx model".into())),
        };
        Ok(Self { rec, backend })
    }

    pub fn transcribe(
        &self,
        samples: &[f32],
        vad_model: Option<&Path>,
        cancel: &AtomicBool,
        on_progress: &mut dyn FnMut(f64),
    ) -> Result<Vec<Segment>, EngineError> {
        let rate = SAMPLE_RATE as f64;
        // Поиск речи — примерно десятая часть всей работы.
        let pieces = speech_pieces(samples, vad_model, cancel, &mut |p| on_progress(p * VAD_SHARE))?;
        let total: usize = pieces.iter().map(|r| r.len()).sum::<usize>().max(1);
        let mut done = 0usize;
        let mut out = Vec::new();

        // Офлайн-модель считает несколько фраз за один проход — так быстрее.
        let batch = match self.rec {
            Recognizer::Offline(_) => BATCH,
            Recognizer::Online(_) => 1,
        };
        for group in pieces.chunks(batch) {
            if cancel.load(Ordering::Relaxed) {
                return Err(EngineError::Cancelled);
            }
            let audio: Vec<&[f32]> = group.iter().map(|r| &samples[r.clone()]).collect();
            let results = match self.rec {
                Recognizer::Offline(r) => decode_offline(r, &audio),
                Recognizer::Online(r) => audio.iter().map(|a| decode_online(r, a)).collect(),
            };
            done += group.iter().map(|r| r.len()).sum::<usize>();
            on_progress(VAD_SHARE + (1.0 - VAD_SHARE) * done as f64 / total as f64);

            for (piece, result) in group.iter().zip(results) {
                let Some(result) = result else { continue };
                if let Some(seg) = self.segment(result, piece.start as f64 / rate, piece.end as f64 / rate) {
                    out.push(seg);
                }
            }
        }
        Ok(out)
    }

    /// Сегмент из ответа модели: текст, время, слова (для разделения по голосам).
    fn segment(&self, r: RawResult, offset: f64, end: f64) -> Option<Segment> {
        let tone = self.backend == Backend::Tone;
        let shift = if tone { TONE_LEFT_PAD + TONE_LATENCY } else { 0.0 };
        let mut tokens: Vec<(f64, f64, Vec<u8>)> = Vec::new();
        for (i, tok) in r.tokens.iter().enumerate() {
            let start = offset + (r.timestamps.get(i).copied().unwrap_or(0.0) - shift).max(0.0);
            let next = r.timestamps.get(i + 1).map(|t| offset + (t - shift).max(0.0)).unwrap_or(end);
            // У GigaAM начало слова помечено «▁», у T-One слова разделяет отдельный пробел.
            let text = tok.replace('▁', " ");
            tokens.push((start.min(end), next.clamp(start.min(end), end), text.into_bytes()));
        }
        let mut words = crate::diarize::words_from_tokens(tokens);
        words.retain(|w| !String::from_utf8_lossy(&w.bytes).trim().is_empty());
        let mut text = r.text.trim().to_string();
        if tone {
            text = tidy_phrase(&text);
        }
        if text.is_empty() {
            return None;
        }
        let start = words.first().map(|w| w.start).unwrap_or(offset);
        let stop = words.last().map(|w| w.end).unwrap_or(end).max(start);
        Some(Segment { start, end: stop, text, speaker: None, words })
    }
}

/// Слева T-One нужна тишина, чтобы «разогреться» (так в примерах sherpa-onnx).
const TONE_LEFT_PAD: f64 = 0.3;
const TONE_RIGHT_PAD: f64 = 0.66;
/// Потоковая модель выдаёт букву с опозданием примерно на столько секунд
/// (замерено на записях с известным началом речи).
const TONE_LATENCY: f64 = 0.35;

struct RawResult {
    text: String,
    tokens: Vec<String>,
    timestamps: Vec<f64>,
}

fn parse_result(json: *const std::os::raw::c_char) -> Option<RawResult> {
    if json.is_null() {
        return None;
    }
    let s = unsafe { CStr::from_ptr(json) }.to_string_lossy().to_string();
    let v: serde_json::Value = serde_json::from_str(&s).ok()?;
    let text = v.get("text")?.as_str()?.to_string();
    let tokens = v
        .get("tokens")
        .and_then(|t| t.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    let timestamps = v
        .get("timestamps")
        .and_then(|t| t.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_f64()).collect())
        .unwrap_or_default();
    Some(RawResult { text, tokens, timestamps })
}

/// Сколько фраз GigaAM считает за один проход.
const BATCH: usize = 4;

fn decode_offline(r: *const so::OfflineRecognizer, pieces: &[&[f32]]) -> Vec<Option<RawResult>> {
    unsafe {
        let streams: Vec<_> = pieces.iter().map(|_| so::SherpaOnnxCreateOfflineStream(r)).collect();
        if streams.iter().any(|s| s.is_null()) {
            streams.iter().filter(|s| !s.is_null()).for_each(|s| so::SherpaOnnxDestroyOfflineStream(*s));
            return pieces.iter().map(|_| None).collect();
        }
        for (s, a) in streams.iter().zip(pieces) {
            so::SherpaOnnxAcceptWaveformOffline(*s, SAMPLE_RATE as i32, a.as_ptr(), a.len() as i32);
        }
        so::SherpaOnnxDecodeMultipleOfflineStreams(r, streams.as_ptr(), streams.len() as i32);
        streams
            .into_iter()
            .map(|s| {
                let json = so::SherpaOnnxGetOfflineStreamResultAsJson(s);
                let out = parse_result(json);
                if !json.is_null() {
                    so::SherpaOnnxDestroyOfflineStreamResultJson(json);
                }
                so::SherpaOnnxDestroyOfflineStream(s);
                out
            })
            .collect()
    }
}

fn decode_online(r: *const so::OnlineRecognizer, audio: &[f32]) -> Option<RawResult> {
    let rate = SAMPLE_RATE as f64;
    let left = vec![0f32; (TONE_LEFT_PAD * rate) as usize];
    let right = vec![0f32; (TONE_RIGHT_PAD * rate) as usize];
    unsafe {
        let stream = so::SherpaOnnxCreateOnlineStream(r);
        if stream.is_null() {
            return None;
        }
        for part in [&left[..], audio, &right[..]] {
            so::SherpaOnnxOnlineStreamAcceptWaveform(stream, SAMPLE_RATE as i32, part.as_ptr(), part.len() as i32);
        }
        so::SherpaOnnxOnlineStreamInputFinished(stream);
        while so::SherpaOnnxIsOnlineStreamReady(r, stream) != 0 {
            so::SherpaOnnxDecodeOnlineStream(r, stream);
        }
        let json = so::SherpaOnnxGetOnlineStreamResultAsJson(r, stream);
        let out = parse_result(json);
        if !json.is_null() {
            so::SherpaOnnxDestroyOnlineStreamResultJson(json);
        }
        so::SherpaOnnxDestroyOnlineStream(stream);
        out
    }
}

/// T-One пишет строчными и без знаков: делаем хотя бы заглавную в начале
/// фразы и точку в конце. Повторный вызов ничего не меняет.
pub fn tidy_phrase(text: &str) -> String {
    let mut out = sentence_case(text.trim());
    if !out.is_empty() && !out.ends_with(['.', '!', '?', '…']) {
        out.push('.');
    }
    out
}

/// Первая буква — заглавная.
fn sentence_case(s: &str) -> String {
    let lead = s.len() - s.trim_start().len();
    let (space, rest) = s.split_at(lead);
    let mut chars = rest.chars();
    match chars.next() {
        Some(c) => {
            let mut out = String::from(space);
            out.extend(c.to_uppercase());
            out.push_str(chars.as_str());
            out
        }
        None => s.to_string(),
    }
}

/// Куски записи с речью, каждый не длиннее MAX_PIECE_SECS.
fn speech_pieces(
    samples: &[f32],
    vad_model: Option<&Path>,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(f64),
) -> Result<Vec<Range<usize>>, EngineError> {
    let rate = SAMPLE_RATE as f64;
    let spans = match vad_model.and_then(|p| vad_spans(samples, p, cancel, on_progress)) {
        Some(spans) => spans,
        None => {
            if cancel.load(Ordering::Relaxed) {
                return Err(EngineError::Cancelled);
            }
            // Без детектора — просто равные куски.
            let step = (MAX_PIECE_SECS as f64 * rate) as usize;
            (0..samples.len()).step_by(step).map(|s| (s as f64 / rate, (s + step).min(samples.len()) as f64 / rate)).collect()
        }
    };
    Ok(merge_spans(&spans, JOIN_GAP_SECS, MAX_PIECE_SECS as f64)
        .into_iter()
        .map(|(a, b)| {
            let s = ((a - PAD_SECS).max(0.0) * rate) as usize;
            let e = (((b + PAD_SECS) * rate) as usize).min(samples.len());
            s..e
        })
        .filter(|r| r.len() > (rate * 0.2) as usize)
        .collect())
}

fn vad_spans(
    samples: &[f32],
    model: &Path,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(f64),
) -> Option<Vec<(f64, f64)>> {
    let opts = VadOptions { min_silence_ms: 300, max_speech_s: MAX_PIECE_SECS, pad_ms: 100 };
    speech_spans(samples, model, &opts, cancel, on_progress)
}

/// Склеивает соседние отрезки с короткой паузой, пока кусок не длиннее `max`.
fn merge_spans(spans: &[(f64, f64)], gap: f64, max: f64) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::new();
    for &(a, b) in spans {
        if b <= a {
            continue;
        }
        match out.last_mut() {
            Some(last) if a - last.1 <= gap && b - last.0 <= max => last.1 = b,
            _ => out.push((a, b)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_close_spans_up_to_limit() {
        let spans = [(0.0, 3.0), (3.5, 8.0), (8.4, 21.0), (21.5, 25.0), (27.0, 30.0)];
        assert_eq!(merge_spans(&spans, 0.8, 20.0), vec![(0.0, 8.0), (8.4, 25.0), (27.0, 30.0)]);
    }

    #[test]
    fn sentence_case_works() {
        assert_eq!(tidy_phrase("привет мир"), "Привет мир.");
        assert_eq!(tidy_phrase("Привет мир."), "Привет мир.");
        assert_eq!(sentence_case("привет мир"), "Привет мир");
        assert_eq!(sentence_case(" ёлка"), " Ёлка");
        assert_eq!(sentence_case(""), "");
    }
}
