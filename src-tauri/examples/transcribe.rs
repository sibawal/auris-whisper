//! Консольная проверка: тот же декодер и движок, что и в приложении.
//!
//!   cargo run --release --example transcribe -- <модель.bin> <аудиофайл> [язык] [--cpu] [--diarize <папка> [число спикеров]]
//!
//! `--diarize` — разделить по голосам; в папке лежат модели сегментации и отпечатков
//! (имена как в каталоге приложения).
//!
//! Детектор речи берётся из src-tauri/resources.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use auris_whisper_lib::{audio, engine};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("использование: transcribe <модель.bin> <аудиофайл> [язык] [--cpu]");
        std::process::exit(2);
    }
    whisper_rs::install_logging_hooks();
    let use_gpu = !args.iter().any(|a| a == "--cpu");
    let lang = args.get(3).filter(|a| !a.starts_with("--")).cloned();
    let diarize_dir = args.iter().position(|a| a == "--diarize").and_then(|i| args.get(i + 1)).map(std::path::PathBuf::from);
    let speakers: Option<u32> =
        args.iter().position(|a| a == "--diarize").and_then(|i| args.get(i + 2)).and_then(|s| s.parse().ok());

    let t0 = Instant::now();
    let samples = match audio::decode_file(Path::new(&args[2])) {
        Ok(s) => s,
        Err(e) => {
            println!("ОШИБКА декодирования: {e}");
            std::process::exit(1);
        }
    };
    let secs = samples.len() as f64 / audio::SAMPLE_RATE as f64;
    println!("декодировано: {} отсчётов = {secs:.2} c за {:.2} c", samples.len(), t0.elapsed().as_secs_f64());

    let vad = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/ggml-silero-v5.1.2.bin");
    let mut eng = engine::Engine::new(Some(vad), Arc::new(AtomicBool::new(false)));
    let t1 = Instant::now();
    if let Err(e) = eng.load(Path::new(&args[1]), use_gpu) {
        println!("ОШИБКА загрузки модели: {}", e.message());
        std::process::exit(1);
    }
    println!(
        "модель загружена за {:.2} c, видеокарта: {} ({:?})",
        t1.elapsed().as_secs_f64(),
        if eng.on_gpu() == Some(true) { engine::gpu_backend_name() } else { "нет, CPU" },
        engine::gpu_devices()
    );

    let t2 = Instant::now();
    let turns = diarize_dir.map(|dir| {
        let t = Instant::now();
        let turns = auris_whisper_lib::diarize::diarize(
            &samples,
            &dir.join(auris_whisper_lib::models::DIARIZE_SEGMENTATION.file),
            &dir.join(auris_whisper_lib::models::DIARIZE_EMBEDDING.file),
            speakers,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .expect("diarization failed");
        let n = turns.iter().map(|t| t.speaker).max().map(|m| m + 1).unwrap_or(0);
        for t in &turns {
            println!("  отрезок {:6.2}–{:6.2}  спикер {}", t.start, t.end, t.speaker + 1);
        }
        println!("разделено по голосам за {:.2} c: спикеров {n}, отрезков {}", t.elapsed().as_secs_f64(), turns.len());
        turns
    });
    let segs = match eng.transcribe(&samples, lang.as_deref(), turns.is_some(), &mut |_| {}) {
        Ok(s) => s,
        Err(e) => {
            println!("ОШИБКА: {}", e.message());
            std::process::exit(1);
        }
    };
    let segs = match &turns {
        Some(t) => auris_whisper_lib::diarize::split_by_speaker(segs, t),
        None => segs,
    };
    let el = t2.elapsed().as_secs_f64();
    println!("распознано за {el:.2} c (×{:.1} от реального времени)", secs / el);
    println!("язык: {}", eng.detected_language().unwrap_or_else(|| "?".into()));
    println!("--- с таймкодами ---\n{}", engine::timestamped_text(&segs));
    if turns.is_some() {
        println!("--- по спикерам ---");
        for s in &segs {
            println!("[{}] Спикер {}: {}", engine::timecode(s.start), s.speaker.map(|x| x + 1).unwrap_or(0), s.text);
        }
    }
    println!("--- сплошным текстом ---\n{}", engine::plain_text(&segs));
}
