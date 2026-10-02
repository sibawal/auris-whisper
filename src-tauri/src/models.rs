//! Каталог моделей и их загрузка. Модели не вшиты в установщик: пользователь
//! выбирает подходящую, и она докачивается с HuggingFace в папку данных приложения.
//! Загрузка продолжается с места обрыва, файл сверяется по SHA-256.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::i18n::tr;

const HF_BASE: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    pub id: &'static str,
    pub file: &'static str,
    pub size: u64,
    #[serde(skip)]
    pub sha256: &'static str,
    /// 1…5 — для полосок «качество» и «скорость» в интерфейсе.
    pub quality: u8,
    pub speed: u8,
    /// Сколько оперативной памяти нужно, ГБ (ориентир).
    pub ram_gb: f32,
}

/// От самой лёгкой до самой точной. Только многоязычные модели — английские
/// `.en` русский не понимают.
pub const CATALOG: &[CatalogModel] = &[
    CatalogModel { id: "tiny", file: "ggml-tiny.bin", size: 77_691_713,
        sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21", quality: 1, speed: 5, ram_gb: 0.4 },
    CatalogModel { id: "base", file: "ggml-base.bin", size: 147_951_465,
        sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe", quality: 2, speed: 5, ram_gb: 0.5 },
    CatalogModel { id: "small", file: "ggml-small.bin", size: 487_601_967,
        sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b", quality: 3, speed: 4, ram_gb: 1.0 },
    CatalogModel { id: "medium", file: "ggml-medium.bin", size: 1_533_763_059,
        sha256: "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208", quality: 4, speed: 2, ram_gb: 2.6 },
    CatalogModel { id: "large-v3-turbo-q5_0", file: "ggml-large-v3-turbo-q5_0.bin", size: 574_041_195,
        sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2", quality: 4, speed: 4, ram_gb: 1.5 },
    CatalogModel { id: "large-v3-turbo", file: "ggml-large-v3-turbo.bin", size: 1_624_555_275,
        sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69", quality: 5, speed: 3, ram_gb: 2.8 },
    CatalogModel { id: "large-v3", file: "ggml-large-v3.bin", size: 3_095_033_483,
        sha256: "64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2", quality: 5, speed: 1, ram_gb: 4.7 },
];

pub fn find(id: &str) -> Option<&'static CatalogModel> {
    CATALOG.iter().find(|m| m.id == id)
}

/// Что посоветовать по умолчанию. На видеокарте turbo-q5_0 — лучший баланс:
/// почти как large-v3 по качеству и в разы быстрее. На одном процессоре
/// разумнее small.
pub fn recommended(has_gpu: bool, total_ram_gb: f64) -> &'static str {
    if !has_gpu {
        return "small";
    }
    if total_ram_gb < 6.0 {
        return "small";
    }
    "large-v3-turbo-q5_0"
}

#[derive(Clone, Serialize)]
pub struct InstalledModel {
    pub id: String,
    pub file: String,
    pub size: u64,
    pub custom: bool,
}

pub fn is_vad_file(name: &str) -> bool {
    name.starts_with("ggml-silero")
}

/// Что лежит в папке моделей: модели из каталога и свои `.bin`.
pub fn installed(dir: &Path) -> Vec<InstalledModel> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".bin") || is_vad_file(&name) {
            continue;
        }
        // metadata() по пути, а не по записи каталога: так символические ссылки
        // на модели с другого диска тоже считаются.
        let size = std::fs::metadata(entry.path()).map(|m| m.len()).unwrap_or(0);
        match CATALOG.iter().find(|m| m.file == name) {
            Some(m) => {
                // Недокачанный файл каталог не считает установленным.
                if size == m.size {
                    out.push(InstalledModel { id: m.id.into(), file: name, size, custom: false });
                }
            }
            None => {
                let id = name.trim_end_matches(".bin").trim_start_matches("ggml-").to_string();
                out.push(InstalledModel { id, file: name, size, custom: true });
            }
        }
    }
    out.sort_by_key(|m| m.size);
    out
}

pub fn model_path(dir: &Path, id: &str) -> Option<PathBuf> {
    installed(dir).into_iter().find(|m| m.id == id).map(|m| dir.join(m.file))
}

// MARK: - Загрузка

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub id: String,
    pub downloaded: u64,
    pub total: u64,
    pub bytes_per_sec: f64,
    /// downloading | verifying | done | error | cancelled
    pub phase: &'static str,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Downloads {
    active: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl Downloads {
    pub fn is_active(&self, id: &str) -> bool {
        self.active.lock().unwrap().contains_key(id)
    }

    pub fn active_ids(&self) -> Vec<String> {
        self.active.lock().unwrap().keys().cloned().collect()
    }

    pub fn cancel(&self, id: &str) {
        if let Some(flag) = self.active.lock().unwrap().get(id) {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// Регистрирует загрузку; `None`, если эта модель уже качается.
    pub fn begin(&self, id: &str) -> Option<Arc<AtomicBool>> {
        let mut map = self.active.lock().unwrap();
        if map.contains_key(id) {
            return None;
        }
        let flag = Arc::new(AtomicBool::new(false));
        map.insert(id.to_string(), flag.clone());
        Some(flag)
    }

    pub fn finish(&self, id: &str) {
        self.active.lock().unwrap().remove(id);
    }
}

/// Качает модель в `dir`. Прогресс отдаётся не чаще пяти раз в секунду.
pub async fn download(
    model: &CatalogModel,
    dir: &Path,
    cancel: Arc<AtomicBool>,
    mut report: impl FnMut(DownloadProgress),
) -> Result<(), String> {
    tokio::fs::create_dir_all(dir).await.map_err(|e| e.to_string())?;
    let target = dir.join(model.file);
    let part = dir.join(format!("{}.part", model.file));

    let progress = |downloaded: u64, bps: f64, phase: &'static str| DownloadProgress {
        id: model.id.into(),
        downloaded,
        total: model.size,
        bytes_per_sec: bps,
        phase,
        error: None,
    };

    // Уже недокачанный кусок: досчитываем хеш и продолжаем с того же места.
    let mut hasher = Sha256::new();
    let mut have: u64 = 0;
    if let Ok(meta) = tokio::fs::metadata(&part).await {
        if meta.len() > 0 && meta.len() < model.size {
            report(progress(0, 0.0, "verifying"));
            let mut f = tokio::fs::File::open(&part).await.map_err(|e| e.to_string())?;
            let mut buf = vec![0u8; 1 << 20];
            loop {
                let n = f.read(&mut buf).await.map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                have += n as u64;
            }
        } else {
            let _ = tokio::fs::remove_file(&part).await;
        }
    }

    let client = reqwest::Client::builder()
        .user_agent(concat!("AurisWhisper/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;

    let url = format!("{HF_BASE}{}", model.file);
    let mut req = client.get(&url);
    if have > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let resp = req.send().await.map_err(net_err)?;
    let status = resp.status();

    let mut file = if have > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT {
        tokio::fs::OpenOptions::new().append(true).open(&part).await.map_err(|e| e.to_string())?
    } else if status.is_success() {
        // Сервер не умеет докачку — начинаем заново.
        hasher = Sha256::new();
        have = 0;
        tokio::fs::File::create(&part).await.map_err(|e| e.to_string())?
    } else {
        return Err(format!("HTTP {status}"));
    };

    let mut stream = resp.bytes_stream();
    let started = Instant::now();
    let start_have = have;
    let mut last_report = Instant::now() - Duration::from_secs(1);

    while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::Relaxed) {
            file.flush().await.ok();
            report(progress(have, 0.0, "cancelled"));
            return Err("cancelled".into());
        }
        let chunk = chunk.map_err(net_err)?;
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        hasher.update(&chunk);
        have += chunk.len() as u64;

        if last_report.elapsed() >= Duration::from_millis(200) {
            let secs = started.elapsed().as_secs_f64().max(0.001);
            report(progress(have, (have - start_have) as f64 / secs, "downloading"));
            last_report = Instant::now();
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);

    if have != model.size {
        return Err(tr(
            "Загрузка оборвалась. Нажмите «Скачать» ещё раз — она продолжится с того же места.",
            "The download was interrupted. Press “Download” again — it will resume where it stopped.",
        ));
    }

    report(progress(have, 0.0, "verifying"));
    let digest = format!("{:x}", hasher.finalize());
    if digest != model.sha256 {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(tr(
            "Файл модели скачался с ошибкой (не совпала контрольная сумма). Попробуйте ещё раз.",
            "The model file is corrupted (checksum mismatch). Please try again.",
        ));
    }
    tokio::fs::rename(&part, &target).await.map_err(|e| e.to_string())?;
    report(progress(have, 0.0, "done"));
    Ok(())
}

fn net_err(e: reqwest::Error) -> String {
    tr(
        "Нет связи с сервером моделей (huggingface.co). Проверьте интернет и попробуйте снова. ",
        "Cannot reach the model server (huggingface.co). Check your connection and try again. ",
    ) + &format!("({e})")
}

/// Импорт своего файла модели (например, скачанного вручную на другом компьютере).
pub fn import(src: &Path, dir: &Path) -> Result<String, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let name = src.file_name().and_then(|n| n.to_str()).ok_or("bad file name")?.to_string();
    if !name.ends_with(".bin") {
        return Err(tr("Нужен файл модели whisper.cpp в формате .bin (ggml).", "Expected a whisper.cpp model file (.bin, ggml)."));
    }
    let mut head = [0u8; 4];
    {
        use std::io::Read;
        let mut f = std::fs::File::open(src).map_err(|e| e.to_string())?;
        f.read_exact(&mut head).map_err(|e| e.to_string())?;
    }
    // ggml-модели whisper начинаются с магического числа 0x67676d6c ("ggml").
    if u32::from_le_bytes(head) != 0x6767_6d6c {
        return Err(tr("Это не модель whisper.cpp (ggml).", "This is not a whisper.cpp (ggml) model."));
    }
    let dst = dir.join(&name);
    if dst != src {
        std::fs::copy(src, &dst).map_err(|e| e.to_string())?;
    }
    Ok(installed(dir).into_iter().find(|m| m.file == name).map(|m| m.id).unwrap_or(name))
}
