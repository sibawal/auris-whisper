// В релизной сборке для Windows — без чёрного окна консоли рядом с приложением.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(target_arch = "x86_64")]
    compat::relaunch_if_needed();
    auris_whisper_lib::run()
}

/// Движок whisper.cpp на x86-64 собран под AVX2/FMA/F16C (процессоры с 2013 года):
/// так он в разы быстрее на процессоре. На более старых машинах (Core 2-го и 3-го
/// поколения, Pentium, Celeron) первая же такая инструкция роняет программу.
/// Поэтому в установщике лежит вторая копия, собранная без них, — её и запускаем.
#[cfg(target_arch = "x86_64")]
mod compat {
    pub fn relaunch_if_needed() {
        if auris_whisper_lib::IS_COMPAT_BUILD || cpu_ok() {
            return;
        }
        let Ok(exe) = std::env::current_exe() else { return };
        let name = if cfg!(windows) { "auris-whisper-compat.exe" } else { "auris-whisper-compat" };
        let compat = exe.with_file_name(name);
        if !compat.is_file() {
            return;
        }
        if std::process::Command::new(compat).args(std::env::args_os().skip(1)).spawn().is_ok() {
            std::process::exit(0);
        }
    }

    fn cpu_ok() -> bool {
        std::arch::is_x86_feature_detected!("avx2")
            && std::arch::is_x86_feature_detected!("fma")
            && std::arch::is_x86_feature_detected!("f16c")
    }
}
