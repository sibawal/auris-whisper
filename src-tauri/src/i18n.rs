//! Две строки — русская и английская. Язык интерфейса держим глобально,
//! чтобы `tr()` можно было звать откуда угодно, в том числе из фоновых потоков.

use std::sync::atomic::{AtomicBool, Ordering};

static ENGLISH: AtomicBool = AtomicBool::new(false);

pub fn set_english(english: bool) {
    ENGLISH.store(english, Ordering::Relaxed);
}

pub fn tr(ru: &str, en: &str) -> String {
    if ENGLISH.load(Ordering::Relaxed) { en } else { ru }.to_string()
}
