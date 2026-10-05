//! Проверка декодера без распознавания: печатает длительность или ошибку.
//!
//!   cargo run --release --example decode -- файл1 [файл2 …]

use auris_whisper_lib::audio;
use std::path::Path;

fn main() {
    let mut failed = 0;
    for f in std::env::args().skip(1) {
        let name = Path::new(&f).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        match audio::decode_file(Path::new(&f)) {
            Ok(s) => println!("OK    {name:32} {:6.2} c", s.len() as f64 / audio::SAMPLE_RATE as f64),
            Err(e) => {
                failed += 1;
                println!("FAIL  {name:32} {e}");
            }
        }
    }
    std::process::exit(if failed > 0 { 1 } else { 0 });
}
