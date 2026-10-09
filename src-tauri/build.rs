fn main() {
    // Linux: библиотеки sherpa-onnx ставятся в /usr/lib/auris-whisper
    // (и туда же внутри AppImage) — бинарник ищет их там.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../lib/auris-whisper");
    }
    tauri_build::build()
}
