// В релизной сборке для Windows — без чёрного окна консоли рядом с приложением.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    auris_whisper_lib::run()
}
