//! Поиск речи детектором Silero (тот же файл, что использует whisper.cpp).
//! Длинную запись прогоняем кусками — ради прогресса и отмены.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use whisper_rs::{WhisperVadContext, WhisperVadContextParams, WhisperVadParams};

use crate::audio::SAMPLE_RATE;

/// Детектор прогоняем кусками по столько секунд.
const CHUNK_SECS: usize = 120;

pub struct VadOptions {
    /// Пауза короче этой не разрывает речь, мс.
    pub min_silence_ms: i32,
    /// Отрезок длиннее этого режется, с.
    pub max_speech_s: f32,
    /// Запас тишины по краям отрезка, мс.
    pub pad_ms: i32,
}

pub fn threads() -> i32 {
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    n.saturating_sub(1).clamp(1, 8) as i32
}

/// Отрезки речи в секундах. `None` — детектор не поднялся или работу отменили.
///
/// Куски по две минуты независимы, поэтому считаются параллельно — у каждого
/// потока свой экземпляр детектора (он крошечный).
pub fn speech_spans(
    samples: &[f32],
    model: &Path,
    opts: &VadOptions,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(f64),
) -> Option<Vec<(f64, f64)>> {
    let model = model.to_str()?;
    let mut vp = WhisperVadParams::new();
    vp.set_threshold(0.5);
    vp.set_min_speech_duration(250);
    vp.set_min_silence_duration(opts.min_silence_ms);
    vp.set_max_speech_duration(opts.max_speech_s);
    vp.set_speech_pad(opts.pad_ms);
    vp.set_samples_overlap(0.0);

    let rate = SAMPLE_RATE as usize;
    let chunk = CHUNK_SECS * rate;
    let starts: Vec<usize> = (0..samples.len()).step_by(chunk).collect();
    let workers = (threads() as usize).min(starts.len()).max(1);
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);

    // Номер куска и найденные в нём отрезки речи.
    type Part = (usize, Vec<(f64, f64)>);
    let parts: Vec<Vec<Part>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut out = Vec::new();
                    let mut cp = WhisperVadContextParams::new();
                    cp.set_n_threads(1);
                    cp.set_use_gpu(false);
                    let Ok(mut ctx) = WhisperVadContext::new(model, cp) else {
                        failed.store(true, Ordering::Relaxed);
                        return out;
                    };
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= starts.len() || cancel.load(Ordering::Relaxed) || failed.load(Ordering::Relaxed) {
                            return out;
                        }
                        let start = starts[i];
                        let end = (start + chunk).min(samples.len());
                        let offset = start as f64 / rate as f64;
                        // Хвост короче секунды детектору не отдаём — в нём нечего искать.
                        if end - start >= rate {
                            match ctx.segments_from_samples(vp, &samples[start..end]) {
                                Ok(segs) => out.push((
                                    i,
                                    segs.map(|s| (offset + s.start as f64 / 100.0, offset + s.end as f64 / 100.0)).collect(),
                                )),
                                Err(_) => failed.store(true, Ordering::Relaxed),
                            }
                        }
                        done.fetch_add(end - start, Ordering::Relaxed);
                    }
                })
            })
            .collect();
        // Прогресс отдаём из этого потока: колбэк не обязан быть потокобезопасным.
        while !handles.iter().all(|h| h.is_finished()) {
            on_progress(done.load(Ordering::Relaxed) as f64 / samples.len().max(1) as f64);
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        handles.into_iter().map(|h| h.join().unwrap_or_default()).collect()
    });
    if failed.load(Ordering::Relaxed) || cancel.load(Ordering::Relaxed) {
        return None;
    }
    on_progress(1.0);
    let mut parts: Vec<Part> = parts.into_iter().flatten().collect();
    parts.sort_by_key(|p| p.0);
    Some(parts.into_iter().flat_map(|p| p.1).collect())
}
