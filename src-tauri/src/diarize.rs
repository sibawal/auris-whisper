//! Разделение по голосам (диаризация) — по схеме 3D-Speaker:
//!
//! 1. детектор речи Silero находит, где говорят;
//! 2. речь режется на окна по 1,5 с с шагом 0,75 с;
//! 3. для каждого окна — голосовой отпечаток (3D-Speaker CAM++ через sherpa-onnx);
//! 4. отпечатки группируются в спикеров (см. cluster.rs);
//! 5. каждому слову распознанного текста достаётся спикер, который говорил
//!    в этот момент, и текст режется на реплики.
//!
//! Раньше здесь был готовый конвейер sherpa-onnx (сегментация pyannote +
//! иерархическая кластеризация «полной связи»). На живых разговорах он находил
//! 5–11 спикеров вместо двух, а при заданном числе отдавал «второму» случайный
//! шум — весь текст доставался одному. И был в несколько раз медленнее.

use std::ffi::CString;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use sherpa_onnx_sys as sys;

use crate::audio::SAMPLE_RATE;
use crate::cluster;
use crate::engine::{Segment, Word};
use crate::i18n::tr;
use crate::vad::{self, VadOptions};

/// Отрезок времени, когда говорил один спикер (секунды).
#[derive(Clone, Debug)]
pub struct Turn {
    pub start: f64,
    pub end: f64,
    pub speaker: usize,
}

/// Окно для голосового отпечатка и шаг между окнами, с.
const WINDOW: f64 = 1.5;
const SHIFT: f64 = 0.75;
/// Короче этого по голосу ничего не понять — такие обрывки пропускаем.
const MIN_WINDOW: f64 = 0.5;
/// Сколько спикеров максимум ищем сами.
const MAX_SPEAKERS: usize = 8;
/// Доля прогресса на поиск речи; остальное — отпечатки.
const VAD_SHARE: f64 = 0.15;

/// Находит, кто когда говорил. `speakers` — сколько людей в записи, если известно.
pub fn diarize(
    samples: &[f32],
    vad_model: Option<&Path>,
    embedding_model: &Path,
    speakers: Option<u32>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<Vec<Turn>, String> {
    let rate = SAMPLE_RATE as f64;
    let total = samples.len() as f64 / rate;

    // 1. Где говорят. Паузы короче 0,2 с не разрывают речь: смена спикера
    // посреди отрезка всё равно найдётся по окнам.
    let opts = VadOptions { min_silence_ms: 200, max_speech_s: 30.0, pad_ms: 50 };
    let spans = match vad_model {
        Some(m) => vad::speech_spans(samples, m, &opts, cancel, &mut |p| progress(p * VAD_SHARE)),
        None => None,
    };
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    let spans = spans.unwrap_or_else(|| vec![(0.0, total)]);

    // 2. Окна.
    let windows = windows(&spans);
    if windows.is_empty() {
        return Ok(Vec::new());
    }

    // 3. Отпечатки.
    let embs = embeddings(samples, &windows, embedding_model, cancel, &mut |p| {
        progress(VAD_SHARE + (1.0 - VAD_SHARE) * p)
    })?;

    // 4. Кто есть кто.
    let labels = cluster::cluster(&embs, speakers.map(|n| n as usize), MAX_SPEAKERS);

    // 5. Окна перекрываются: граница между соседними — посередине между их центрами.
    Ok(turns(samples, &windows, &labels))
}

/// Окно отпечатка: отрезок записи и номер отрезка речи, из которого оно взято.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Window {
    start: f64,
    end: f64,
    span: usize,
}

fn windows(spans: &[(f64, f64)]) -> Vec<Window> {
    let mut out = Vec::new();
    for (i, &(a, b)) in spans.iter().enumerate() {
        let len = b - a;
        if len < MIN_WINDOW {
            continue;
        }
        if len <= WINDOW {
            out.push(Window { start: a, end: b, span: i });
            continue;
        }
        let mut s = a;
        while s + WINDOW < b {
            out.push(Window { start: s, end: s + WINDOW, span: i });
            s += SHIFT;
        }
        // Последнее окно — вплотную к концу отрезка.
        out.push(Window { start: b - WINDOW, end: b, span: i });
    }
    out
}

fn turns(samples: &[f32], windows: &[Window], labels: &[usize]) -> Vec<Turn> {
    let centre = |w: &Window| (w.start + w.end) / 2.0;
    // Граница между соседними окнами. Тот же спикер — посередине между центрами;
    // сменился — в самом тихом месте рядом: люди почти всегда меняются в паузе.
    let boundary = |a: usize, b: usize| {
        let (ca, cb) = (centre(&windows[a]), centre(&windows[b]));
        if labels[a] == labels[b] {
            (ca + cb) / 2.0
        } else {
            quietest(samples, ca - SHIFT / 2.0, cb + SHIFT / 2.0).unwrap_or((ca + cb) / 2.0)
        }
    };
    let mut out: Vec<Turn> = Vec::new();
    for (i, w) in windows.iter().enumerate() {
        let left = match i.checked_sub(1) {
            Some(p) if windows[p].span == w.span => boundary(p, i),
            _ => w.start,
        };
        let right = match windows.get(i + 1) {
            Some(n) if n.span == w.span => boundary(i, i + 1),
            _ => w.end,
        };
        match out.last_mut() {
            Some(t) if t.speaker == labels[i] && (left - t.end).abs() < 1e-6 => t.end = right,
            _ => out.push(Turn { start: left, end: right, speaker: labels[i] }),
        }
    }
    out
}

/// Середина самого тихого места на отрезке [from, to], с: кусочки по 20 мс,
/// и если тишина длится дольше одного кусочка — её середина.
fn quietest(samples: &[f32], from: f64, to: f64) -> Option<f64> {
    let rate = SAMPLE_RATE as f64;
    let frame = (0.02 * rate) as usize;
    let step = (0.01 * rate) as usize;
    let a = ((from.max(0.0) * rate) as usize).min(samples.len());
    let b = ((to.max(0.0) * rate) as usize).min(samples.len());
    let energy: Vec<(usize, f32)> =
        (a..b.saturating_sub(frame)).step_by(step).map(|i| (i, samples[i..i + frame].iter().map(|x| x * x).sum())).collect();
    let min = energy.iter().map(|e| e.1).fold(f32::MAX, f32::min);
    let quiet = |e: f32| e <= min * 1.5 + 1e-9;
    // Самая длинная подряд идущая тишина.
    let (mut best, mut run_start, mut best_len) = (None, 0, 0);
    for (k, e) in energy.iter().enumerate() {
        if !quiet(e.1) {
            run_start = k + 1;
        } else if k + 1 - run_start > best_len {
            best_len = k + 1 - run_start;
            best = Some((run_start, k));
        }
    }
    let (first, last) = best?;
    Some(((energy[first].0 + energy[last].0 + frame) / 2) as f64 / rate)
}

/// Отпечатки окон. Окна короткие, и одна модель плохо загружает несколько ядер,
/// поэтому работают несколько экземпляров модели, каждый в своём потоке.
fn embeddings(
    samples: &[f32],
    windows: &[Window],
    model: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<Vec<Vec<f32>>, String> {
    let model = CString::new(model.to_string_lossy().as_bytes())
        .map_err(|_| tr("Не удалось разделить запись по голосам.", "Could not split the recording by speaker."))?;
    let workers = (vad::threads() as usize).clamp(1, MAX_EMBED_WORKERS).min(windows.len().div_ceil(8));
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let broken = AtomicBool::new(false);
    let mut out: Vec<Vec<f32>> = vec![Vec::new(); windows.len()];

    let results: Vec<Vec<(usize, Vec<f32>)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut got = Vec::new();
                    let Some(ex) = Extractor::new(&model) else {
                        broken.store(true, Ordering::Relaxed);
                        return got;
                    };
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= windows.len() || cancel.load(Ordering::Relaxed) || broken.load(Ordering::Relaxed) {
                            return got;
                        }
                        let w = &windows[i];
                        let rate = SAMPLE_RATE as f64;
                        let a = ((w.start * rate) as usize).min(samples.len());
                        let b = ((w.end * rate) as usize).min(samples.len());
                        got.push((i, ex.embed(&samples[a..b])));
                        done.fetch_add(1, Ordering::Relaxed);
                    }
                })
            })
            .collect();
        while !handles.iter().all(|h| h.is_finished()) {
            progress(done.load(Ordering::Relaxed) as f64 / windows.len() as f64);
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        handles.into_iter().map(|h| h.join().unwrap_or_default()).collect()
    });
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    if broken.load(Ordering::Relaxed) {
        return Err(tr(
            "Модель разделения по голосам повреждена — удалите её в окне моделей и скачайте заново.",
            "The speaker model is damaged — delete it in the models window and download again.",
        ));
    }
    for (i, v) in results.into_iter().flatten() {
        out[i] = v;
    }
    progress(1.0);
    Ok(out)
}

/// Больше стольких копий модели отпечатков не держим (каждая ~60 МБ памяти).
const MAX_EMBED_WORKERS: usize = 4;

/// Модель голосовых отпечатков (одна копия на поток).
struct Extractor {
    ptr: *const sys::SpeakerEmbeddingExtractor,
    dim: usize,
}

impl Extractor {
    fn new(model: &CString) -> Option<Self> {
        let cpu = CString::new("cpu").unwrap();
        let config = sys::SpeakerEmbeddingExtractorConfig { model: model.as_ptr(), num_threads: 1, debug: 0, provider: cpu.as_ptr() };
        let ptr = unsafe { sys::SherpaOnnxCreateSpeakerEmbeddingExtractor(&config) };
        if ptr.is_null() {
            return None;
        }
        let dim = unsafe { sys::SherpaOnnxSpeakerEmbeddingExtractorDim(ptr) }.max(0) as usize;
        Some(Self { ptr, dim })
    }

    /// Нормированный отпечаток куска записи; при сбое — нулевой вектор.
    fn embed(&self, piece: &[f32]) -> Vec<f32> {
        let mut v = vec![0f32; self.dim];
        unsafe {
            let stream = sys::SherpaOnnxSpeakerEmbeddingExtractorCreateStream(self.ptr);
            if !stream.is_null() {
                sys::SherpaOnnxOnlineStreamAcceptWaveform(stream, SAMPLE_RATE as i32, piece.as_ptr(), piece.len() as i32);
                sys::SherpaOnnxOnlineStreamInputFinished(stream);
                if sys::SherpaOnnxSpeakerEmbeddingExtractorIsReady(self.ptr, stream) != 0 {
                    let e = sys::SherpaOnnxSpeakerEmbeddingExtractorComputeEmbedding(self.ptr, stream);
                    if !e.is_null() {
                        v.copy_from_slice(std::slice::from_raw_parts(e, self.dim));
                        sys::SherpaOnnxSpeakerEmbeddingExtractorDestroyEmbedding(e);
                    }
                }
                sys::SherpaOnnxDestroyOnlineStream(stream);
            }
        }
        cluster::normalize(&mut v);
        v
    }
}

impl Drop for Extractor {
    fn drop(&mut self) {
        unsafe { sys::SherpaOnnxDestroySpeakerEmbeddingExtractor(self.ptr) }
    }
}

/// Кто говорил в момент `t`: отрезок, который его накрывает, иначе ближайший.
fn speaker_at(turns: &[Turn], t: f64) -> Option<usize> {
    if turns.is_empty() {
        return None;
    }
    if let Some(turn) = turns.iter().find(|x| x.start <= t && t <= x.end) {
        return Some(turn.speaker);
    }
    turns
        .iter()
        .min_by(|a, b| {
            let da = (a.start - t).abs().min((a.end - t).abs());
            let db = (b.start - t).abs().min((b.end - t).abs());
            da.total_cmp(&db)
        })
        .map(|x| x.speaker)
}

/// Меньше стольких слов подряд — не реплика, а погрешность времени на границе.
const MIN_RUN_WORDS: usize = 3;

/// Режет сегменты whisper на реплики по спикерам. Слова получают спикера по
/// своей середине. Время слов у whisper приблизительное, и первое слово новой
/// реплики часто «залезает» к предыдущему спикеру — поэтому обрывки короче
/// трёх слов присоединяются к соседней реплике.
pub fn split_by_speaker(segments: Vec<Segment>, turns: &[Turn]) -> Vec<Segment> {
    let mut out = Vec::new();
    for seg in segments {
        if seg.words.is_empty() {
            let mid = (seg.start + seg.end) / 2.0;
            out.push(Segment { speaker: speaker_at(turns, mid), ..seg });
            continue;
        }
        let labels: Vec<Option<usize>> =
            seg.words.iter().map(|w| speaker_at(turns, (w.start + w.end) / 2.0)).collect();

        // Отрезки подряд идущих слов одного спикера: (спикер, сколько слов).
        let mut runs: Vec<(Option<usize>, usize)> = Vec::new();
        for l in labels {
            match runs.last_mut() {
                Some((sp, n)) if *sp == l => *n += 1,
                _ => runs.push((l, 1)),
            }
        }
        // Короткие обрывки сливаем с соседями, пока они есть.
        while runs.len() > 1 {
            let Some(i) = runs.iter().position(|r| r.1 < MIN_RUN_WORDS) else { break };
            let target = if i == 0 {
                1
            } else if i == runs.len() - 1 || runs[i - 1].1 >= runs[i + 1].1 {
                i - 1
            } else {
                i + 1
            };
            runs[target].1 += runs[i].1;
            runs.remove(i);
            // Соседи с одним спикером склеиваются.
            let mut k = 1;
            while k < runs.len() {
                if runs[k].0 == runs[k - 1].0 {
                    runs[k - 1].1 += runs[k].1;
                    runs.remove(k);
                } else {
                    k += 1;
                }
            }
        }

        let mut i = 0;
        for (speaker, n) in runs {
            let words = &seg.words[i..i + n];
            i += n;
            let bytes: Vec<u8> = words.iter().flat_map(|w| w.bytes.iter().copied()).collect();
            let text = String::from_utf8_lossy(&bytes).trim().to_string();
            if text.is_empty() {
                continue;
            }
            out.push(Segment { start: words[0].start, end: words[n - 1].end, text, speaker, words: Vec::new() });
        }
    }
    out
}

/// Склеивает слова из байтов токенов: новое слово начинается с пробела.
pub fn words_from_tokens(tokens: Vec<(f64, f64, Vec<u8>)>) -> Vec<Word> {
    let mut words: Vec<Word> = Vec::new();
    for (start, end, bytes) in tokens {
        match words.last_mut() {
            Some(w) if !bytes.starts_with(b" ") => {
                w.bytes.extend_from_slice(&bytes);
                w.end = end;
            }
            _ => words.push(Word { start, end, bytes }),
        }
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(s: f64, e: f64, t: &str) -> Word {
        Word { start: s, end: e, bytes: t.as_bytes().to_vec() }
    }

    #[test]
    fn splits_segment_at_speaker_change() {
        let turns = vec![
            Turn { start: 0.0, end: 2.0, speaker: 0 },
            Turn { start: 2.0, end: 4.0, speaker: 1 },
        ];
        let seg = Segment {
            start: 0.0,
            end: 4.0,
            text: String::new(),
            speaker: None,
            words: vec![
                word(0.0, 0.5, " Привет,"),
                word(0.6, 1.2, " как"),
                word(1.3, 1.9, " дела?"),
                word(2.1, 2.6, " Хорошо,"),
                word(2.7, 3.2, " спасибо"),
                word(3.3, 3.9, " большое."),
            ],
        };
        let out = split_by_speaker(vec![seg], &turns);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].text, "Привет, как дела?");
        assert_eq!(out[0].speaker, Some(0));
        assert_eq!(out[1].text, "Хорошо, спасибо большое.");
        assert_eq!(out[1].speaker, Some(1));
    }

    #[test]
    fn leading_word_joins_its_phrase() {
        // Первое слово сегмента второго спикера по времени попало к первому.
        let turns = vec![
            Turn { start: 0.0, end: 5.2, speaker: 0 },
            Turn { start: 5.3, end: 9.0, speaker: 1 },
        ];
        let seg = Segment {
            start: 5.0,
            end: 9.0,
            text: String::new(),
            speaker: None,
            words: vec![word(5.0, 5.2, " Good"), word(5.3, 6.0, " morning."), word(6.1, 7.0, " Actually"), word(7.1, 8.0, " fine.")],
        };
        let out = split_by_speaker(vec![seg], &turns);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].speaker, Some(1));
        assert_eq!(out[0].text, "Good morning. Actually fine.");
    }

    #[test]
    fn smooths_single_word_flip() {
        let turns = vec![
            Turn { start: 0.0, end: 1.0, speaker: 0 },
            Turn { start: 1.0, end: 1.2, speaker: 1 },
            Turn { start: 1.2, end: 3.0, speaker: 0 },
        ];
        let seg = Segment {
            start: 0.0,
            end: 3.0,
            text: String::new(),
            speaker: None,
            words: vec![word(0.0, 0.9, " один"), word(1.0, 1.2, " два"), word(1.3, 2.9, " три")],
        };
        let out = split_by_speaker(vec![seg], &turns);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "один два три");
    }

    #[test]
    fn windows_cover_speech() {
        let w = windows(&[(0.0, 0.3), (1.0, 2.0), (3.0, 6.2)]);
        // Обрывок 0,3 с пропущен, отрезок 1 с — одно окно, 3,2 с — окна с шагом 0,75 с.
        assert_eq!(w[0], Window { start: 1.0, end: 2.0, span: 1 });
        assert!(w[1..].iter().all(|x| x.span == 2 && (x.end - x.start - WINDOW).abs() < 1e-9));
        assert_eq!(w.last().unwrap().end, 6.2);
    }

    #[test]
    fn turns_split_between_window_centres() {
        let w = windows(&[(0.0, 3.0), (4.0, 5.0)]);
        // Окна 0–1,5; 0,75–2,25; 1,5–3,0 и 4–5. Голос везде, кроме паузы на 1,6 с —
        // смена спикера между 2-м и 3-м окном должна встать в неё.
        let rate = SAMPLE_RATE as usize;
        let mut audio: Vec<f32> = (0..5 * rate).map(|i| ((i as f32) * 0.05).sin() * 0.5).collect();
        audio[(1.55 * rate as f64) as usize..(1.65 * rate as f64) as usize].fill(0.0);
        let t = turns(&audio, &w, &[0, 0, 1, 1]);
        assert_eq!(t.len(), 3);
        assert_eq!((t[0].start, t[0].speaker), (0.0, 0));
        assert!((t[0].end - 1.6).abs() < 0.03, "{}", t[0].end);
        assert_eq!((t[1].end, t[1].speaker), (3.0, 1));
        assert_eq!((t[2].start, t[2].end, t[2].speaker), (4.0, 5.0, 1));
    }

    #[test]
    fn words_join_subword_tokens() {
        let w = words_from_tokens(vec![(0.0, 0.2, b" \xd0\x9f\xd1\x80".to_vec()), (0.2, 0.4, "ивет".as_bytes().to_vec()), (0.5, 0.7, b" all".to_vec())]);
        assert_eq!(w.len(), 2);
        assert_eq!(String::from_utf8_lossy(&w[0].bytes), " Привет");
        assert_eq!(w[0].end, 0.4);
    }
}
