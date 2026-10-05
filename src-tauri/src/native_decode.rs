//! Запасные декодеры: если symphonia не открыла файл, пробуем то, что умеет система.
//!
//! - macOS — AVFoundation (тот же путь, что был в версии 1.x на Swift): видео с iPhone
//!   в HEVC, AAC 5.1, AC-3/E-AC-3, ALAC и всё, что открывает QuickTime;
//! - Windows — Media Foundation: mp4/mov, wmv/wma, avi, ts, AC-3 и прочее,
//!   что играет «Кино и ТВ»;
//! - везде — ffmpeg, если он установлен (на Linux это основной запасной путь).

use std::path::{Path, PathBuf};

use crate::audio::SAMPLE_RATE;

/// Достаточно ли того, что вернул декодер (меньше 0,1 с — считаем неудачей).
fn usable(samples: &[f32]) -> bool {
    samples.len() >= (SAMPLE_RATE / 10) as usize
}

/// Пробует системный декодер, потом ffmpeg. `None` — никто не справился.
pub fn decode(path: &Path) -> Option<Vec<f32>> {
    if let Ok(s) = platform::decode(path) {
        if usable(&s) {
            return Some(s);
        }
    }
    match ffmpeg_decode(path) {
        Ok(s) if usable(&s) => Some(s),
        _ => None,
    }
}

/// Есть ли ffmpeg — чтобы подсказать установить его, если файл не открылся.
pub fn has_ffmpeg() -> bool {
    ffmpeg_path().is_some()
}

// MARK: - ffmpeg

fn ffmpeg_path() -> Option<PathBuf> {
    // Для тестов: проверить системный декодер без подстраховки ffmpeg.
    if std::env::var_os("AURIS_NO_FFMPEG").is_some() {
        return None;
    }
    let exe = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    // Приложения из Finder не получают PATH из оболочки, поэтому смотрим
    // и в типичные места установки.
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    for extra in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/snap/bin"] {
        dirs.push(PathBuf::from(extra));
    }
    dirs.into_iter().map(|d| d.join(exe)).find(|p| p.is_file())
}

fn ffmpeg_decode(path: &Path) -> Result<Vec<f32>, String> {
    use std::process::{Command, Stdio};
    let ff = ffmpeg_path().ok_or("ffmpeg not found")?;
    let mut cmd = Command::new(ff);
    cmd.args(["-nostdin", "-v", "error", "-i"])
        .arg(path)
        .args(["-vn", "-ac", "1", "-ar", "16000", "-f", "f32le", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(out.stdout.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect())
}

// MARK: - macOS: AVFoundation

#[cfg(target_os = "macos")]
mod platform {
    use std::path::Path;
    use std::ptr::NonNull;

    use objc2::rc::{autoreleasepool, Retained};
    use objc2::runtime::AnyObject;
    use objc2_av_foundation::{AVAssetReader, AVAssetReaderStatus, AVAssetReaderTrackOutput, AVMediaTypeAudio, AVURLAsset};
    use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};

    /// AVAssetReader сразу отдаёт 16 кГц, моно, float — как в версии 1.x.
    pub fn decode(path: &Path) -> Result<Vec<f32>, String> {
        let path = path.to_str().ok_or("bad path")?.to_owned();
        autoreleasepool(|_| unsafe {
            let url = NSURL::fileURLWithPath(&NSString::from_str(&path));
            let asset = AVURLAsset::URLAssetWithURL_options(&url, None);
            let media = AVMediaTypeAudio.ok_or("AVMediaTypeAudio")?;
            #[allow(deprecated)]
            let tracks = asset.tracksWithMediaType(media);
            let track = tracks.firstObject().ok_or("no audio track")?;

            let reader = AVAssetReader::assetReaderWithAsset_error(&asset)
                .map_err(|e| e.localizedDescription().to_string())?;

            // Ключи AVAudioSettings — строки, совпадающие со своими именами.
            let keys = [
                "AVFormatIDKey",
                "AVSampleRateKey",
                "AVNumberOfChannelsKey",
                "AVLinearPCMBitDepthKey",
                "AVLinearPCMIsFloatKey",
                "AVLinearPCMIsBigEndianKey",
                "AVLinearPCMIsNonInterleaved",
            ]
            .map(NSString::from_str);
            let values: [Retained<NSNumber>; 7] = [
                NSNumber::new_u32(u32::from_be_bytes(*b"lpcm")),
                NSNumber::new_f64(crate::audio::SAMPLE_RATE as f64),
                NSNumber::new_i32(1),
                NSNumber::new_i32(32),
                NSNumber::new_bool(true),
                NSNumber::new_bool(false),
                NSNumber::new_bool(false),
            ];
            let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
            let value_refs: Vec<&AnyObject> = values.iter().map(|v| -> &AnyObject { v }).collect();
            let settings = NSDictionary::from_slices(&key_refs, &value_refs);

            let output = AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(&track, Some(&settings));
            reader.addOutput(&output);
            if !reader.startReading() {
                return Err(reader.error().map(|e| e.localizedDescription().to_string()).unwrap_or_default());
            }

            let mut samples: Vec<f32> = Vec::new();
            while let Some(sb) = output.copyNextSampleBuffer() {
                if let Some(block) = sb.data_buffer() {
                    let len = block.data_length();
                    if len >= 4 {
                        let start = samples.len();
                        samples.resize(start + len / 4, 0.0);
                        let dst = NonNull::new(samples[start..].as_mut_ptr().cast()).unwrap();
                        if block.copy_data_bytes(0, (len / 4) * 4, dst) != 0 {
                            samples.truncate(start);
                        }
                    }
                }
                sb.invalidate();
            }
            if reader.status() == AVAssetReaderStatus::Failed {
                return Err(reader.error().map(|e| e.localizedDescription().to_string()).unwrap_or_default());
            }
            Ok(samples)
        })
    }
}

// MARK: - Windows: Media Foundation

#[cfg(target_os = "windows")]
mod platform {
    use std::path::Path;

    use windows::core::HSTRING;
    use windows::Win32::Media::MediaFoundation::{
        IMFMediaType, IMFSample, IMFSourceReader, MFAudioFormat_Float, MFCreateMediaType, MFCreateSourceReaderFromURL,
        MFMediaType_Audio, MFShutdown, MFStartup, MFSTARTUP_NOSOCKET, MF_MT_AUDIO_NUM_CHANNELS,
        MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_SOURCE_READERF_ENDOFSTREAM,
        MF_SOURCE_READER_ALL_STREAMS, MF_SOURCE_READER_FIRST_AUDIO_STREAM, MF_VERSION,
    };
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

    use crate::audio::MonoSink;

    pub fn decode(path: &Path) -> Result<Vec<f32>, String> {
        unsafe {
            // Декодер работает в своём потоке расшифровки — COM здесь свой.
            let com = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).map_err(|e| e.to_string())?;
            let result = read(path);
            let _ = MFShutdown();
            if com.is_ok() {
                CoUninitialize();
            }
            result
        }
    }

    unsafe fn read(path: &Path) -> Result<Vec<f32>, String> {
        let err = |e: windows::core::Error| e.message().to_string();
        let audio = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;

        let reader: IMFSourceReader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None).map_err(err)?;
        reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false).map_err(err)?;
        reader.SetStreamSelection(audio, true).map_err(err)?;

        // Просим несжатый float; частоту и каналы оставляем родными — их приводим сами.
        let want: IMFMediaType = MFCreateMediaType().map_err(err)?;
        want.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).map_err(err)?;
        want.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float).map_err(err)?;
        reader.SetCurrentMediaType(audio, None, &want).map_err(err)?;

        let got = reader.GetCurrentMediaType(audio).map_err(err)?;
        let rate = got.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).map_err(err)?;
        let channels = got.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).map_err(err)?.max(1) as usize;

        let mut sink = MonoSink::default();
        loop {
            let mut flags = 0u32;
            let mut sample: Option<IMFSample> = None;
            reader.ReadSample(audio, 0, None, Some(&mut flags), None, Some(&mut sample)).map_err(err)?;
            if let Some(sample) = sample {
                let buffer = sample.ConvertToContiguousBuffer().map_err(err)?;
                let mut ptr = std::ptr::null_mut();
                let mut len = 0u32;
                buffer.Lock(&mut ptr, None, Some(&mut len)).map_err(err)?;
                let data = std::slice::from_raw_parts(ptr as *const f32, len as usize / 4);
                let pushed = sink.push_interleaved(data, channels, rate);
                let _ = buffer.Unlock();
                pushed?;
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
        }
        sink.finish()
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    pub fn decode(_path: &std::path::Path) -> Result<Vec<f32>, String> {
        Err("no system decoder".into())
    }
}
