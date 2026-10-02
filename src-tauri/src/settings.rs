//! Настройки, которые переживают перезапуск. Лежат в settings.json в папке
//! конфигурации приложения.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// "ru" | "en"; пусто — выбрать по языку системы при первом запуске.
    pub ui_lang: Option<String>,
    /// Язык распознавания: "auto" или код whisper ("ru", "en", …).
    pub language: String,
    pub timestamps: bool,
    pub auto_save: bool,
    /// Выбранная модель (id из каталога или имя своего файла).
    pub model: Option<String>,
    pub use_gpu: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { ui_lang: None, language: "auto".into(), timestamps: false, auto_save: true, model: None, use_gpu: true }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }

    pub fn english(&self) -> bool {
        self.ui_lang.as_deref() == Some("en")
    }
}
