//! Проверка диктовки без окна: тот же модуль записи, что в приложении.
//! Пишет N секунд с микрофона по умолчанию и, если дана модель, расшифровывает.
//!
//!   cargo run --release --example record -- <секунды> [модель.bin] [язык]
//!
//! В CI на Linux микрофоном служит виртуальный источник PulseAudio,
//! в который проигрывается тестовая фраза.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use auris_whisper_lib::{audio, engine, recorder};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let secs: f64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(5.0);

    for d in recorder::list_devices() {
        println!("микрофон: {}{}", d.name, if d.is_default { " (по умолчанию)" } else { "" });
    }

    let loudest = Arc::new(AtomicU32::new(0));
    let silent = Arc::new(AtomicBool::new(false));
    let (l, s) = (loudest.clone(), silent.clone());
    let rec = match recorder::Recording::start(None, move |lvl| {
        if lvl.level > f32::from_bits(l.load(Ordering::Relaxed)) {
            l.store(lvl.level.to_bits(), Ordering::Relaxed);
        }
        s.store(lvl.silent, Ordering::Relaxed);
    }) {
        Ok(r) => r,
        Err(e) => {
            println!("ОШИБКА старта записи: {e}");
            std::process::exit(1);
        }
    };
    std::thread::sleep(Duration::from_secs_f64(secs));
    let samples = match rec.stop() {
        Ok(s) => s,
        Err(e) => {
            println!("ОШИБКА записи: {e}");
            std::process::exit(1);
        }
    };
    println!(
        "записано {:.2} c, максимальный уровень {:.2}, «молчит» в конце: {}",
        samples.len() as f64 / audio::SAMPLE_RATE as f64,
        f32::from_bits(loudest.load(Ordering::Relaxed)),
        silent.load(Ordering::Relaxed)
    );

    let Some(model) = args.get(2) else { return };
    whisper_rs::install_logging_hooks();
    let vad = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/ggml-silero-v5.1.2.bin");
    let mut eng = engine::Engine::new(Some(vad), Arc::new(AtomicBool::new(false)));
    if let Err(e) = eng.load(Path::new(model), true) {
        println!("ОШИБКА загрузки модели: {}", e.message());
        std::process::exit(1);
    }
    match eng.transcribe(&samples, args.get(3).map(String::as_str), false, &mut |_| {}) {
        Ok(segs) => println!("текст: {}", engine::plain_text(&segs)),
        Err(e) => {
            println!("ОШИБКА распознавания: {}", e.message());
            std::process::exit(1);
        }
    }
}
