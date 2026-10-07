//! Разделение по голосам (диаризация).
//!
//! sherpa-onnx: модель pyannote находит, где кто-то говорит, голосовые отпечатки
//! (3D-Speaker CAM++) сравнивают фрагменты между собой, кластеризация собирает
//! их в спикеров. Потом каждому слову из whisper достаётся спикер, который
//! говорил в этот момент, и текст режется на реплики.

use std::ffi::{c_void, CString};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use sherpa_onnx_sys as sys;

use crate::engine::{Segment, Word};
use crate::i18n::tr;

/// Отрезок времени, когда говорил один спикер (секунды).
#[derive(Clone, Debug)]
pub struct Turn {
    pub start: f64,
    pub end: f64,
    pub speaker: usize,
}

type ProgressCallback = unsafe extern "C" fn(i32, i32, *mut c_void) -> i32;

// В C-библиотеке есть вариант с прогрессом, в Rust-привязках — нет.
extern "C" {
    fn SherpaOnnxOfflineSpeakerDiarizationProcessWithCallback(
        sd: *const sys::OfflineSpeakerDiarization,
        samples: *const f32,
        n: i32,
        callback: Option<ProgressCallback>,
        arg: *mut c_void,
    ) -> *const sys::OfflineSpeakerDiarizationResult;
}

struct CallbackData<'a> {
    progress: &'a mut dyn FnMut(f64),
}

unsafe extern "C" fn progress_trampoline(done: i32, total: i32, arg: *mut c_void) -> i32 {
    if !arg.is_null() && total > 0 {
        let data = &mut *(arg as *mut CallbackData);
        (data.progress)(done as f64 / total as f64);
    }
    0
}

/// Находит, кто когда говорил. `speakers` — сколько людей в записи, если известно.
pub fn diarize(
    samples: &[f32],
    segmentation_model: &Path,
    embedding_model: &Path,
    speakers: Option<u32>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<Vec<Turn>, String> {
    let fail = || tr("Не удалось разделить запись по голосам.", "Could not split the recording by speaker.");
    let seg = CString::new(segmentation_model.to_string_lossy().as_bytes()).map_err(|_| fail())?;
    let emb = CString::new(embedding_model.to_string_lossy().as_bytes()).map_err(|_| fail())?;
    let cpu = CString::new("cpu").unwrap();
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8) as i32;

    let config = sys::OfflineSpeakerDiarizationConfig {
        segmentation: sys::OfflineSpeakerSegmentationModelConfig {
            pyannote: sys::OfflineSpeakerSegmentationPyannoteModelConfig { model: seg.as_ptr(), window_shift_ratio: 0.1 },
            num_threads: threads,
            debug: 0,
            provider: cpu.as_ptr(),
        },
        embedding: sys::SpeakerEmbeddingExtractorConfig {
            model: emb.as_ptr(),
            num_threads: threads,
            debug: 0,
            provider: cpu.as_ptr(),
        },
        clustering: sys::FastClusteringConfig {
            // Число спикеров известно — кластеров ровно столько; иначе — порог похожести.
            num_clusters: speakers.map(|n| n as i32).unwrap_or(-1),
            threshold: 0.5,
            compute_confidence: 0,
        },
        min_duration_on: 0.3,
        min_duration_off: 0.5,
    };

    unsafe {
        let sd = sys::SherpaOnnxCreateOfflineSpeakerDiarization(&config);
        if sd.is_null() {
            return Err(tr(
                "Модели разделения по голосам повреждены — удалите их в окне моделей и скачайте заново.",
                "The speaker models are damaged — delete them in the models window and download again.",
            ));
        }
        let mut data = CallbackData { progress };
        let result = SherpaOnnxOfflineSpeakerDiarizationProcessWithCallback(
            sd,
            samples.as_ptr(),
            samples.len() as i32,
            Some(progress_trampoline),
            &mut data as *mut CallbackData as *mut c_void,
        );
        sys::SherpaOnnxDestroyOfflineSpeakerDiarization(sd);
        if cancel.load(Ordering::Relaxed) {
            if !result.is_null() {
                sys::SherpaOnnxOfflineSpeakerDiarizationDestroyResult(result);
            }
            return Err("cancelled".into());
        }
        if result.is_null() {
            return Err(fail());
        }
        let n = sys::SherpaOnnxOfflineSpeakerDiarizationResultGetNumSegments(result).max(0) as usize;
        let segs = sys::SherpaOnnxOfflineSpeakerDiarizationResultSortByStartTime(result);
        let mut turns = Vec::with_capacity(n);
        if !segs.is_null() {
            for s in std::slice::from_raw_parts(segs, n) {
                turns.push(Turn { start: s.start as f64, end: s.end as f64, speaker: s.speaker.max(0) as usize });
            }
            sys::SherpaOnnxOfflineSpeakerDiarizationDestroySegment(segs);
        }
        sys::SherpaOnnxOfflineSpeakerDiarizationDestroyResult(result);
        Ok(renumber(turns))
    }
}

/// Номера спикеров — по порядку первого появления: «Спикер 1» — тот, кто заговорил первым.
fn renumber(mut turns: Vec<Turn>) -> Vec<Turn> {
    let mut order: Vec<usize> = Vec::new();
    for t in &turns {
        if !order.contains(&t.speaker) {
            order.push(t.speaker);
        }
    }
    for t in &mut turns {
        t.speaker = order.iter().position(|&s| s == t.speaker).unwrap_or(0);
    }
    turns
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
    fn renumbers_by_first_appearance() {
        let t = renumber(vec![
            Turn { start: 0.0, end: 1.0, speaker: 3 },
            Turn { start: 1.0, end: 2.0, speaker: 0 },
            Turn { start: 2.0, end: 3.0, speaker: 3 },
        ]);
        assert_eq!(t.iter().map(|x| x.speaker).collect::<Vec<_>>(), vec![0, 1, 0]);
    }

    #[test]
    fn words_join_subword_tokens() {
        let w = words_from_tokens(vec![(0.0, 0.2, b" \xd0\x9f\xd1\x80".to_vec()), (0.2, 0.4, "ивет".as_bytes().to_vec()), (0.5, 0.7, b" all".to_vec())]);
        assert_eq!(w.len(), 2);
        assert_eq!(String::from_utf8_lossy(&w[0].bytes), " Привет");
        assert_eq!(w[0].end, 0.4);
    }
}
