//! Обёртка над whisper.cpp. Контекст модели живёт, пока не сменят модель,
//! поэтому вторая и последующие расшифровки стартуют мгновенно.

use std::ffi::c_void;
use std::os::raw::c_int;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Serialize;
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState, WhisperSysContext,
    WhisperSysState, WhisperVadParams,
};

use crate::audio::SAMPLE_RATE;
use crate::i18n::tr;
use crate::models::Backend;
use crate::ru_asr::RuModel;

#[derive(Clone, Debug, Serialize)]
pub struct Segment {
    pub start: f64, // секунды
    pub end: f64,
    pub text: String,
    /// Номер спикера (с нуля), если запись разделяли по голосам.
    pub speaker: Option<usize>,
    /// Слова с временем — нужны только чтобы разрезать сегмент по спикерам.
    #[serde(skip)]
    pub words: Vec<Word>,
}

/// Слово (точнее, набор токенов до следующего пробела) с временем в секундах.
#[derive(Clone, Debug)]
pub struct Word {
    pub start: f64,
    pub end: f64,
    pub bytes: Vec<u8>,
}

/// Чем считаем: показываем в полосе нагрузки.
pub fn gpu_backend_name() -> &'static str {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "Metal"
    } else if cfg!(any(target_os = "windows", target_os = "linux")) {
        "Vulkan"
    } else {
        "CPU"
    }
}

/// Видеокарты, которые видит ggml (Metal, Vulkan). Опрашиваем через реестр
/// бэкендов: он ловит ошибки инициализации Vulkan, и на машине без драйвера
/// мы просто получим пустой список, а не падение.
pub fn gpu_devices() -> Vec<String> {
    use whisper_rs::whisper_rs_sys as sys;
    let mut out = Vec::new();
    unsafe {
        for i in 0..sys::ggml_backend_dev_count() {
            let dev = sys::ggml_backend_dev_get(i);
            if dev.is_null() {
                continue;
            }
            let kind = sys::ggml_backend_dev_type(dev);
            if kind == sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_GPU
                || kind == sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_IGPU
            {
                let desc = sys::ggml_backend_dev_description(dev);
                if !desc.is_null() {
                    out.push(std::ffi::CStr::from_ptr(desc).to_string_lossy().trim().to_string());
                }
            }
        }
    }
    out
}

/// Есть ли вообще чем ускоряться.
pub fn gpu_available() -> bool {
    !gpu_devices().is_empty()
}

struct Loaded {
    path: PathBuf,
    use_gpu: bool,
    on_gpu: bool,
    // Порядок важен: состояние ссылается на контекст и должно умереть раньше.
    state: WhisperState,
    _ctx: WhisperContext,
}

pub struct Engine {
    loaded: Option<Loaded>,
    /// Русская модель (GigaAM, T-One). Одновременно в памяти только одна модель.
    ru: Option<(PathBuf, RuModel)>,
    vad_model: Option<PathBuf>,
    cancel: Arc<AtomicBool>,
    last_lang: Option<String>,
}

/// Данные для колбэков whisper.cpp: живут на стеке на время одного прогона.
struct CallbackData<'a> {
    cancel: &'a AtomicBool,
    progress: &'a mut dyn FnMut(f64),
}

unsafe extern "C" fn progress_trampoline(
    _: *mut WhisperSysContext,
    _: *mut WhisperSysState,
    progress: c_int,
    user_data: *mut c_void,
) {
    if user_data.is_null() {
        return;
    }
    let data = &mut *(user_data as *mut CallbackData);
    (data.progress)(progress as f64 / 100.0);
}

unsafe extern "C" fn abort_trampoline(user_data: *mut c_void) -> bool {
    if user_data.is_null() {
        return false;
    }
    let data = &*(user_data as *const CallbackData);
    data.cancel.load(Ordering::Relaxed)
}

pub enum EngineError {
    Cancelled,
    Failed(String),
}

impl EngineError {
    pub fn message(&self) -> String {
        match self {
            EngineError::Cancelled => tr("Отменено.", "Cancelled."),
            EngineError::Failed(m) => m.clone(),
        }
    }
}

impl Engine {
    pub fn new(vad_model: Option<PathBuf>, cancel: Arc<AtomicBool>) -> Self {
        Self { loaded: None, ru: None, vad_model, cancel, last_lang: None }
    }

    pub fn is_loaded_with(&self, path: &Path, backend: Backend, use_gpu: bool) -> bool {
        match backend {
            Backend::Whisper => matches!(&self.loaded, Some(l) if l.path == path && l.use_gpu == use_gpu),
            _ => matches!(&self.ru, Some((p, _)) if p == path),
        }
    }

    /// Считает ли загруженная модель на видеокарте.
    pub fn on_gpu(&self) -> Option<bool> {
        if self.ru.is_some() {
            return Some(false);
        }
        self.loaded.as_ref().map(|l| l.on_gpu)
    }

    pub fn unload(&mut self) {
        self.loaded = None;
        self.ru = None;
    }

    /// Детектор речи Silero, если он есть в установке.
    pub fn vad_model(&self) -> Option<&Path> {
        self.vad_model.as_deref()
    }

    pub fn detected_language(&self) -> Option<String> {
        self.last_lang.clone()
    }

    /// Загружает модель. Если видеокарта не поднялась — тихо уходим на процессор.
    pub fn load(&mut self, path: &Path, backend: Backend, use_gpu: bool) -> Result<(), EngineError> {
        if self.is_loaded_with(path, backend, use_gpu) {
            return Ok(());
        }
        self.unload();
        if backend != Backend::Whisper {
            self.ru = Some((path.to_path_buf(), RuModel::load(backend, path)?));
            return Ok(());
        }

        let path_str = path.to_str().ok_or_else(|| EngineError::Failed("bad model path".into()))?;
        let try_load = |gpu: bool| -> Option<(WhisperContext, WhisperState)> {
            let mut cp = WhisperContextParameters::default();
            cp.use_gpu(gpu);
            cp.flash_attn(true);
            let ctx = WhisperContext::new_with_params(path_str, cp).ok()?;
            let state = ctx.create_state().ok()?;
            Some((ctx, state))
        };

        let (ctx, state, on_gpu) = match use_gpu.then(|| try_load(true)).flatten() {
            Some((c, s)) => (c, s, gpu_available()),
            None => match try_load(false) {
                Some((c, s)) => (c, s, false),
                None => {
                    return Err(EngineError::Failed(tr(
                        "Не удалось загрузить модель. Возможно, файл повреждён — удалите и скачайте её заново.",
                        "Could not load the model. The file may be damaged — delete it and download again.",
                    )))
                }
            },
        };
        self.loaded = Some(Loaded { path: path.to_path_buf(), use_gpu, on_gpu, state, _ctx: ctx });
        Ok(())
    }

    /// Публичный вход: режет длинное аудио на куски и склеивает результат.
    ///
    /// Whisper иногда застревает: окно перестаёт двигаться вперёд, и модель до конца
    /// файла повторяет одну фразу. Внутри одного вызова из этого не выбраться, поэтому
    /// длинную запись обрабатываем частями — срыв портит максимум один кусок,
    /// а следующий стартует с чистого листа.
    pub fn transcribe(
        &mut self,
        samples: &[f32],
        language: Option<&str>,
        words: bool,
        on_progress: &mut dyn FnMut(f64),
    ) -> Result<Vec<Segment>, EngineError> {
        self.last_lang = None;

        if let Some((_, ru)) = &self.ru {
            let segments = ru.transcribe(samples, self.vad_model.as_deref(), &self.cancel, on_progress)?;
            self.last_lang = Some("ru".into());
            return Ok(segments);
        }

        let rate = SAMPLE_RATE as usize;
        let chunk_target = 5 * 60 * rate; // куски примерно по 5 минут
        if samples.len() <= chunk_target + 60 * rate {
            return self.transcribe_chunk(samples, language, 0.0, words, on_progress);
        }

        let bounds = chunk_bounds(samples, chunk_target, rate);
        let n = bounds.len() as f64;
        let mut out = Vec::new();

        for (index, range) in bounds.iter().enumerate() {
            let offset = range.start as f64 / rate as f64;
            let piece = &samples[range.clone()];
            let base = index as f64 / n;
            let span = 1.0 / n;
            let mut sub = |p: f64| on_progress(base + p * span);

            let mut segments = self.transcribe_chunk(piece, language, 0.0, words, &mut sub)?;

            // Кусок сорвался в повтор — пробуем ещё раз, с другой температурой.
            if longest_repeat_run(&segments) >= 5 {
                if let Ok(retry) = self.transcribe_chunk(piece, language, 0.4, words, &mut sub) {
                    if longest_repeat_run(&retry) < longest_repeat_run(&segments) {
                        segments = retry;
                    }
                }
            }

            out.extend(segments.into_iter().map(|s| Segment {
                start: s.start + offset,
                end: s.end + offset,
                text: s.text,
                speaker: None,
                words: s
                    .words
                    .into_iter()
                    .map(|w| Word { start: w.start + offset, end: w.end + offset, bytes: w.bytes })
                    .collect(),
            }));
        }
        Ok(collapse_repeats(out, 3))
    }

    fn transcribe_chunk(
        &mut self,
        samples: &[f32],
        language: Option<&str>,
        temperature: f32,
        words: bool,
        on_progress: &mut dyn FnMut(f64),
    ) -> Result<Vec<Segment>, EngineError> {
        let loaded = self.loaded.as_mut().ok_or_else(|| EngineError::Failed("model is not loaded".into()))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 5 });
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        params.set_n_threads(threads.saturating_sub(1).clamp(2, 8) as c_int);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_print_special(false);
        params.set_translate(false);
        params.set_no_timestamps(false);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true); // глушим «неречевые» токены — меньше мусора и петель
        params.set_no_context(true); // не тащить прошлый текст вперёд: иначе петля кормит сама себя
        params.set_temperature(temperature);
        params.set_temperature_inc(0.2); // фолбэк при плохой уверенности
        params.set_entropy_thold(2.4); // сорвался в повтор → перевыбор с другой температурой
        params.set_logprob_thold(-1.0);
        params.set_no_speech_thold(0.6);
        // Время отдельных токенов — чтобы резать фразы на реплики по спикерам.
        params.set_token_timestamps(words);

        match language {
            Some(lang) if lang != "auto" => params.set_language(Some(lang)),
            _ => {
                params.set_language(None); // whisper сам определит язык на лету
                params.set_detect_language(false);
            }
        }

        // Детектор речи (Silero): на вход декодеру идут только куски с речью.
        // Тишина, музыка и шум — главный источник галлюцинаций и зацикливания.
        let vad_path = self.vad_model.as_ref().and_then(|p| p.to_str().map(str::to_owned));
        if let Some(vad) = vad_path.as_deref() {
            params.set_vad_model_path(Some(vad));
            let mut vp = WhisperVadParams::new();
            vp.set_threshold(0.5);
            vp.set_min_speech_duration(250);
            vp.set_min_silence_duration(300);
            vp.set_max_speech_duration(30.0);
            vp.set_speech_pad(200);
            vp.set_samples_overlap(0.2);
            params.set_vad_params(vp);
            params.enable_vad(true);
        }

        let mut data = CallbackData { cancel: &self.cancel, progress: on_progress };
        let data_ptr = &mut data as *mut CallbackData as *mut c_void;
        unsafe {
            params.set_progress_callback(Some(progress_trampoline));
            params.set_progress_callback_user_data(data_ptr);
            params.set_abort_callback(Some(abort_trampoline));
            params.set_abort_callback_user_data(data_ptr);
        }

        let result = loaded.state.full(params, samples);
        if self.cancel.load(Ordering::Relaxed) {
            return Err(EngineError::Cancelled);
        }
        result.map_err(|e| EngineError::Failed(tr("Ошибка распознавания: ", "Transcription failed: ") + &e.to_string()))?;

        let eot = loaded._ctx.token_eot();
        let state = &loaded.state;
        let lang_id = state.full_lang_id_from_state();
        if lang_id >= 0 {
            self.last_lang = whisper_rs::get_lang_str(lang_id).map(str::to_owned);
        }

        let mut out = Vec::new();
        for seg in state.as_iter() {
            let text = seg.to_str_lossy().map(|s| s.trim().to_string()).unwrap_or_default();
            if text.is_empty() {
                continue;
            }
            let start = seg.start_timestamp() as f64 / 100.0;
            let end = seg.end_timestamp() as f64 / 100.0;
            let words = if words { segment_words(&seg, eot, start, end) } else { Vec::new() };
            out.push(Segment { start, end, text, speaker: None, words });
        }
        Ok(out)
    }
}

/// Слова сегмента со временем на шкале исходной записи.
///
/// whisper.cpp пересчитывает с учётом детектора речи только границы сегментов,
/// а время токенов остаётся на «сжатой» шкале без пауз. Поэтому положение
/// токена внутри сегмента берём относительное и растягиваем на настоящие границы.
fn segment_words(seg: &whisper_rs::WhisperSegment<'_>, eot: i32, start: f64, end: f64) -> Vec<Word> {
    let mut tokens: Vec<(f64, f64, Vec<u8>)> = Vec::new();
    for i in 0..seg.n_tokens() {
        let Some(tok) = seg.get_token(i) else { continue };
        if tok.token_id() >= eot {
            continue; // служебные: начало, метки времени, конец
        }
        let data = tok.token_data();
        let bytes = tok.to_bytes().map(|b| b.to_vec()).unwrap_or_default();
        if bytes.is_empty() {
            continue;
        }
        tokens.push((data.t0 as f64, data.t1.max(data.t0) as f64, bytes));
    }
    let (Some(first), Some(last)) = (tokens.first().map(|t| t.0), tokens.last().map(|t| t.1)) else {
        return Vec::new();
    };
    let span = (last - first).max(1.0);
    let len = (end - start).max(0.0);
    let at = |t: f64| start + ((t - first) / span).clamp(0.0, 1.0) * len;
    crate::diarize::words_from_tokens(tokens.into_iter().map(|(a, b, bytes)| (at(a), at(b), bytes)).collect())
}

/// Границы кусков: режем по самому тихому месту рядом с целевой точкой,
/// чтобы не разорвать слово.
pub fn chunk_bounds(samples: &[f32], target: usize, rate: usize) -> Vec<std::ops::Range<usize>> {
    let mut bounds = Vec::new();
    let mut start = 0;
    let search = 15 * rate; // ищем тишину в ±15 с от точки реза
    let frame = rate / 10; // окно 100 мс

    while start < samples.len() {
        let nominal = start + target;
        if nominal + 30 * rate >= samples.len() {
            bounds.push(start..samples.len());
            break;
        }
        let mut best_cut = nominal;
        let mut best_energy = f32::MAX;
        let mut i = (start + rate).max(nominal.saturating_sub(search));
        let upper = (samples.len() - frame).min(nominal + search);
        while i < upper {
            let sum: f32 = samples[i..i + frame].iter().step_by(8).map(|v| v.abs()).sum();
            if sum < best_energy {
                best_energy = sum;
                best_cut = i + frame / 2;
            }
            i += frame;
        }
        bounds.push(start..best_cut);
        start = best_cut;
    }
    bounds
}

fn norm(s: &str) -> String {
    s.trim().to_lowercase()
}

/// Длина самой длинной цепочки одинаковых подряд идущих сегментов.
pub fn longest_repeat_run(segments: &[Segment]) -> usize {
    let (mut best, mut run) = (0, 0);
    let mut prev = String::new();
    for s in segments {
        let key = norm(&s.text);
        run = if key == prev { run + 1 } else { 1 };
        prev = key;
        best = best.max(run);
    }
    best
}

/// Whisper иногда срывается в петлю и повторяет одну фразу десятки раз.
/// VAD и фолбэк по температуре гасят почти всё, это последний рубеж:
/// `limit` и больше одинаковых подряд — оставляем одну.
pub fn collapse_repeats(segments: Vec<Segment>, limit: usize) -> Vec<Segment> {
    let mut out = Vec::with_capacity(segments.len());
    let mut run: Vec<Segment> = Vec::new();
    let mut run_key = String::new();

    let flush = |run: &mut Vec<Segment>, out: &mut Vec<Segment>| {
        if run.len() >= limit {
            out.push(run[0].clone());
        } else {
            out.append(run);
        }
        run.clear();
    };

    for seg in segments {
        let key = norm(&seg.text);
        if !run.is_empty() && key == run_key {
            run.push(seg);
        } else {
            flush(&mut run, &mut out);
            run_key = key;
            run.push(seg);
        }
    }
    flush(&mut run, &mut out);
    out
}

// MARK: - Форматирование результата

pub fn timecode(seconds: f64) -> String {
    let total = seconds.max(0.0).floor() as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

pub fn plain_text(segments: &[Segment]) -> String {
    segments.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" ").replace("  ", " ")
}

pub fn timestamped_text(segments: &[Segment]) -> String {
    segments
        .iter()
        .map(|s| format!("[{} → {}]  {}", timecode(s.start), timecode(s.end), s.text))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(t: &str) -> Segment {
        Segment { start: 0.0, end: 1.0, text: t.into(), speaker: None, words: Vec::new() }
    }

    #[test]
    fn collapses_long_runs_only() {
        let s = vec![seg("a"), seg("b"), seg("b"), seg("c"), seg("c"), seg("c"), seg("c"), seg("d")];
        let out: Vec<_> = collapse_repeats(s, 3).into_iter().map(|s| s.text).collect();
        assert_eq!(out, ["a", "b", "b", "c", "d"]);
    }

    #[test]
    fn repeat_run() {
        assert_eq!(longest_repeat_run(&[seg("x"), seg("X "), seg("y")]), 2);
    }

    #[test]
    fn chunks_cover_everything() {
        let rate = 100;
        let samples = vec![0.1f32; rate * 60 * 23];
        let b = chunk_bounds(&samples, 5 * 60 * rate, rate);
        assert_eq!(b.first().unwrap().start, 0);
        assert_eq!(b.last().unwrap().end, samples.len());
        for w in b.windows(2) {
            assert_eq!(w[0].end, w[1].start);
        }
    }

    #[test]
    fn timecodes() {
        assert_eq!(timecode(75.9), "01:15");
        assert_eq!(timecode(3725.0), "01:02:05");
    }
}
