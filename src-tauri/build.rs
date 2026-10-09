fn main() {
    // Linux: sherpa-onnx подключён как .so. Путь поиска, который прописывает
    // sherpa-onnx-sys, до наших бинарников не доходит — задаём сами.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        // Установленное приложение: .so лежат в /usr/lib/auris-whisper
        // (и туда же внутри AppImage); при запуске из папки сборки — рядом,
        // их туда копирует sherpa-onnx-sys.
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN/../lib/auris-whisper:$ORIGIN");
        // Консольные примеры запускаются из папки сборки.
        println!("cargo:rerun-if-env-changed=SHERPA_ONNX_LIB_DIR");
        if let Ok(dir) = std::env::var("SHERPA_ONNX_LIB_DIR") {
            println!("cargo:rustc-link-arg-examples=-Wl,-rpath,{dir}");
        }
    }
    tauri_build::build()
}
