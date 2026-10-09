//! Проверка разделения по голосам: печатает отрезки JSON-ом и время работы.
//!
//!   cargo run --release --example diarize -- <папка моделей> <аудио> [число спикеров]

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use auris_whisper_lib::{audio, diarize, models};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("использование: diarize <папка моделей> <аудио> [число спикеров]");
        std::process::exit(2);
    }
    whisper_rs::install_logging_hooks();
    let dir = Path::new(&args[1]);
    let speakers: Option<u32> = args.get(3).and_then(|s| s.parse().ok());
    let samples = audio::decode_file(Path::new(&args[2])).expect("decode");
    let t = Instant::now();
    let turns = diarize::diarize(
        &samples,
        Some(&Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/ggml-silero-v5.1.2.bin")),
        &dir.join(models::DIARIZE_EMBEDDING.file),
        speakers,
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .expect("diarize");
    let secs = t.elapsed().as_secs_f64();
    let json: Vec<String> = turns.iter().map(|t| format!("[{:.3},{:.3},{}]", t.start, t.end, t.speaker)).collect();
    println!("{{\"seconds\":{secs:.3},\"turns\":[{}]}}", json.join(","));
}
