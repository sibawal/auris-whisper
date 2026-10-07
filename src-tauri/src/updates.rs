//! Обновления по воздуху. При запуске приложение смотрит latest.json последнего
//! релиза на GitHub; если там версия новее — интерфейс предлагает её поставить.
//! Пакет обновления подписан ключом проекта, без верной подписи он не ставится.
//!
//! macOS — новая .app заменяет текущую; Windows — установщик в тихом режиме;
//! Linux AppImage — новый файл заменяет текущий. Установленный из .deb пакет
//! обновляет пакетный менеджер, поэтому для него — ссылка на страницу релиза.

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_updater::{Update, UpdaterExt};

pub const RELEASES_PAGE: &str = "https://github.com/sibawal/auris-whisper/releases/latest";

/// Найденное обновление ждёт, пока пользователь нажмёт «Установить».
#[derive(Default)]
pub struct Pending(Mutex<Option<Update>>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    version: String,
    current: String,
    notes: String,
    can_install: bool,
    page: &'static str,
}

#[derive(Clone, Serialize)]
struct UpdateProgress {
    downloaded: u64,
    total: Option<u64>,
}

/// Можно ли поставить обновление прямо из приложения.
fn can_install() -> bool {
    !(cfg!(target_os = "linux") && std::env::var_os("APPIMAGE").is_none())
}

#[tauri::command]
pub async fn check_update(app: AppHandle, pending: State<'_, Pending>) -> Result<Option<UpdateInfo>, String> {
    let update = app.updater().map_err(|e| e.to_string())?.check().await.map_err(|e| e.to_string())?;
    let Some(update) = update else { return Ok(None) };
    let info = UpdateInfo {
        version: update.version.clone(),
        current: update.current_version.clone(),
        notes: update.body.clone().unwrap_or_default(),
        can_install: can_install(),
        page: RELEASES_PAGE,
    };
    *pending.0.lock().unwrap() = Some(update);
    Ok(Some(info))
}

/// Скачивает, ставит и перезапускает приложение. Прогресс — событием «update-progress».
#[tauri::command]
pub async fn install_update(app: AppHandle, pending: State<'_, Pending>) -> Result<(), String> {
    let update = pending.0.lock().unwrap().take().ok_or("no pending update")?;
    let mut downloaded = 0u64;
    let progress_app = app.clone();
    update
        .download_and_install(
            move |chunk, total| {
                downloaded += chunk as u64;
                let _ = progress_app.emit("update-progress", UpdateProgress { downloaded, total });
            },
            || {},
        )
        .await
        .map_err(|e| e.to_string())?;
    app.restart();
}
