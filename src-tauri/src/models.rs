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

/// Чем считается модель.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// whisper.cpp: один файл .bin, любые языки.
    Whisper,
    /// sherpa-onnx, офлайн-модель (GigaAM): папка с файлами .onnx.
    Gigaam,
    /// sherpa-onnx, потоковая модель (T-One).
    Tone,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    pub id: &'static str,
    /// Для whisper — имя файла; для моделей из нескольких файлов — имя папки.
    pub file: &'static str,
    /// Общий размер всех файлов.
    pub size: u64,
    #[serde(skip)]
    pub sha256: &'static str,
    pub backend: Backend,
    /// Модель понимает только этот язык.
    pub lang: Option<&'static str>,
    /// Файлы модели, если их несколько (лежат в папке `file`).
    #[serde(skip)]
    pub files: &'static [RemoteFile],
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
        sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21", quality: 1, speed: 5, ram_gb: 0.4, backend: Backend::Whisper, lang: None, files: &[] },
    CatalogModel { id: "base", file: "ggml-base.bin", size: 147_951_465,
        sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe", quality: 2, speed: 5, ram_gb: 0.5, backend: Backend::Whisper, lang: None, files: &[] },
    CatalogModel { id: "small", file: "ggml-small.bin", size: 487_601_967,
        sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b", quality: 3, speed: 4, ram_gb: 1.0, backend: Backend::Whisper, lang: None, files: &[] },
    CatalogModel { id: "medium", file: "ggml-medium.bin", size: 1_533_763_059,
        sha256: "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208", quality: 4, speed: 2, ram_gb: 2.6, backend: Backend::Whisper, lang: None, files: &[] },
    CatalogModel { id: "large-v3-turbo-q5_0", file: "ggml-large-v3-turbo-q5_0.bin", size: 574_041_195,
        sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2", quality: 4, speed: 4, ram_gb: 1.5, backend: Backend::Whisper, lang: None, files: &[] },
    CatalogModel { id: "large-v3-turbo", file: "ggml-large-v3-turbo.bin", size: 1_624_555_275,
        sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69", quality: 5, speed: 3, ram_gb: 2.8, backend: Backend::Whisper, lang: None, files: &[] },
    CatalogModel { id: "large-v3", file: "ggml-large-v3.bin", size: 3_095_033_483,
        sha256: "64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2", quality: 5, speed: 1, ram_gb: 4.7, backend: Backend::Whisper, lang: None, files: &[] },
    CatalogModel { id: "gigaam-v3", file: "gigaam-v3-rnnt-punct", size: 231_897_202, sha256: "",
        quality: 5, speed: 5, ram_gb: 1.0, backend: Backend::Gigaam, lang: Some("ru"), files: GIGAAM_V3 },
    CatalogModel { id: "t-one", file: "t-one", size: 144_193_904, sha256: "",
        quality: 4, speed: 5, ram_gb: 0.6, backend: Backend::Tone, lang: Some("ru"), files: T_ONE },
];

/// GigaAM v3 от Сбера (MIT): RNN-T с пунктуацией и заглавными буквами, обучена
/// на 700 тыс. часов русской речи. Перевод в ONNX — проект sherpa-onnx.
macro_rules! gigaam_url {
    ($f:literal) => {
        concat!("https://huggingface.co/csukuangfj/sherpa-onnx-nemo-transducer-punct-giga-am-v3-russian-2025-12-16/resolve/main/", $f)
    };
}
const GIGAAM_V3: &[RemoteFile] = &[
    RemoteFile { id: "gigaam-v3", file: "encoder.int8.onnx", size: 224_570_820,
        sha256: "369f35a71bf288d3b8e0391fabd8dba5f2314088d440bca474056b7b4b6e66bf", url: gigaam_url!("encoder.int8.onnx") },
    RemoteFile { id: "gigaam-v3", file: "decoder.onnx", size: 4_600_132,
        sha256: "38fc7475443ea2a26f63211ca350f73ac50fff824ab7a3876ee2bd610c53bbc4", url: gigaam_url!("decoder.onnx") },
    RemoteFile { id: "gigaam-v3", file: "joiner.onnx", size: 2_712_896,
        sha256: "602ff7017a93311aad34df1437c8d7f49911353c13d6eae7a6ee7b041339465c", url: gigaam_url!("joiner.onnx") },
    RemoteFile { id: "gigaam-v3", file: "tokens.txt", size: 13_354,
        sha256: "39abae20e692998290c574e606f11a9edef2902a1995463fcff63d1490cf22b7", url: gigaam_url!("tokens.txt") },
];

/// T-One от Т-Банка (Apache-2.0): потоковая CTC-модель, 70 млн параметров,
/// 80 тыс. часов русской речи. Пишет без знаков препинания.
macro_rules! t_one_url {
    ($f:literal) => {
        concat!("https://huggingface.co/csukuangfj/sherpa-onnx-streaming-t-one-russian-2025-09-08/resolve/main/", $f)
    };
}
const T_ONE: &[RemoteFile] = &[
    RemoteFile { id: "t-one", file: "model.onnx", size: 144_193_702,
        sha256: "5ded080e2a6c86ecc11bcb0902d77524eb3e8b0844cb0c0754347f5aafb4dabc", url: t_one_url!("model.onnx") },
    RemoteFile { id: "t-one", file: "tokens.txt", size: 202,
        sha256: "27f7b3ba2096c572375fba1a6b29af1f80d86e08a329940612908112695f97e0", url: t_one_url!("tokens.txt") },
];

/// Файл, который можно скачать: модель whisper или вспомогательная модель.
#[derive(Clone)]
pub struct RemoteFile {
    pub id: &'static str,
    pub file: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    pub url: &'static str,
}

impl CatalogModel {
    pub fn remote(&self) -> RemoteFile {
        RemoteFile { id: self.id, file: self.file, size: self.size, sha256: self.sha256, url: "" }
    }
}

/// Разделение по голосам: сегментация pyannote 3.0 (MIT) и голосовые отпечатки
/// 3D-Speaker CAM++, обучены на китайской и английской речи, но голоса различают
/// независимо от языка (Apache-2.0).
pub const DIARIZE_SEGMENTATION: RemoteFile = RemoteFile {
    id: "diarize-segmentation",
    file: "pyannote-segmentation-3.0.onnx",
    size: 5_992_913,
    sha256: "220ad67ca923bef2fa91f2390c786097bf305bceb5e261d4af67b38e938e1079",
    url: "https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0/resolve/main/model.onnx",
};
pub const DIARIZE_EMBEDDING: RemoteFile = RemoteFile {
    id: "diarize-embedding",
    file: "3dspeaker-campplus-zh-en-advanced.onnx",
    size: 28_281_164,
    sha256: "aa3cfc16963a10586a9393f5035d6d6b57e98d358b347f80c2a30bf4f00ceba2",
    url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx",
};

/// Все ли файлы набора скачаны целиком.
pub fn files_ready(dir: &Path, files: &[RemoteFile]) -> bool {
    files.iter().all(|f| std::fs::metadata(dir.join(f.file)).map(|m| m.len() == f.size).unwrap_or(false))
}

/// Установлены ли обе модели разделения по голосам.
pub fn diarize_ready(dir: &Path) -> bool {
    files_ready(dir, &[DIARIZE_SEGMENTATION, DIARIZE_EMBEDDING])
}

/// Чем считать модель с таким id. Свои файлы .bin — всегда whisper.
pub fn backend_of(id: &str) -> Backend {
    find(id).map(|m| m.backend).unwrap_or(Backend::Whisper)
}

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
        match CATALOG.iter().find(|m| m.file == name && m.files.is_empty()) {
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
    // Модели из нескольких файлов лежат каждая в своей папке.
    for m in CATALOG.iter().filter(|m| !m.files.is_empty()) {
        if files_ready(&dir.join(m.file), m.files) {
            out.push(InstalledModel { id: m.id.into(), file: m.file.into(), size: m.size, custom: false });
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
    model: &RemoteFile,
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

    let url = if model.url.is_empty() { format!("{HF_BASE}{}", model.file) } else { model.url.to_string() };
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
