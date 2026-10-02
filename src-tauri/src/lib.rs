//! Auris Whisper — офлайн-расшифровка речи. Окно на Tauri, движок whisper.cpp.
//!
//! Интерфейс (ui/) общается с этим кодом через команды `invoke` и события:
//!   job               — ход расшифровки (файл, этап, прогресс, результат, ошибка)
//!   rec-level         — уровень и длительность записи с микрофона
//!   download-progress — загрузка модели
//!   models-changed    — список моделей поменялся
//!   stats             — полоса нагрузки, раз в секунду
//!   open-files        — файлы, открытые через «Открыть с помощью» / двойной клик

pub mod audio;
pub mod engine;
pub mod i18n;
pub mod models;
pub mod monitor;
pub mod recorder;
pub mod settings;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_clipboard_manager::ClipboardExt;

use engine::{Engine, EngineError, Segment};
use i18n::tr;
use settings::Settings;

const VAD_FILE: &str = "ggml-silero-v5.1.2.bin";

struct AppState {
    settings: Mutex<Settings>,
    settings_path: PathBuf,
    models_dir: PathBuf,
    engine: Arc<Mutex<Engine>>,
    cancel: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    recording: Mutex<Option<recorder::Recording>>,
    downloads: Arc<models::Downloads>,
    pending_open: Mutex<Vec<String>>,
    ui_ready: AtomicBool,
    system: SystemInfo,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SystemInfo {
    os: &'static str,
    arch: &'static str,
    gpu_backend: &'static str,
    gpu_devices: Vec<String>,
    total_ram_gb: f64,
    cores: usize,
    recommended: &'static str,
    version: &'static str,
    whisper_version: &'static str,
}

// MARK: - События расшифровки

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum JobEvent {
    #[serde(rename_all = "camelCase")]
    File { index: usize, total: usize, name: String },
    #[serde(rename_all = "camelCase")]
    Phase { phase: &'static str, name: String, audio_seconds: f64, recording: bool },
    Progress { value: f64 },
    #[serde(rename_all = "camelCase")]
    Result {
        name: String,
        text: String,
        header: bool,
        elapsed: f64,
        audio_seconds: f64,
        language: Option<String>,
        saved_to: Option<String>,
        on_gpu: bool,
    },
    #[serde(rename_all = "camelCase")]
    Error { name: String, message: String, no_model: bool },
    Done { cancelled: bool },
}

fn emit_job(app: &AppHandle, ev: JobEvent) {
    let _ = app.emit("job", ev);
}

/// Откуда брать звук для одной задачи очереди.
enum Source {
    File(PathBuf),
    /// Надиктованное: отсчёты 16 кГц и базовое имя файлов для автосохранения.
    Recording(Vec<f32>, String),
}

// MARK: - Команды

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Bootstrap {
    settings: Settings,
    catalog: &'static [models::CatalogModel],
    installed: Vec<models::InstalledModel>,
    downloading: Vec<String>,
    system: SystemInfo,
    models_dir: String,
    pending_files: Vec<String>,
}

#[tauri::command]
fn bootstrap(state: State<'_, AppState>) -> Bootstrap {
    state.ui_ready.store(true, Ordering::Relaxed);
    Bootstrap {
        settings: state.settings.lock().unwrap().clone(),
        catalog: models::CATALOG,
        installed: models::installed(&state.models_dir),
        downloading: state.downloads.active_ids(),
        system: state.system.clone(),
        models_dir: state.models_dir.to_string_lossy().into(),
        pending_files: std::mem::take(&mut *state.pending_open.lock().unwrap()),
    }
}

#[tauri::command]
fn save_settings(state: State<'_, AppState>, settings: Settings) {
    i18n::set_english(settings.english());
    settings.save(&state.settings_path);
    *state.settings.lock().unwrap() = settings;
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelsState {
    installed: Vec<models::InstalledModel>,
    downloading: Vec<String>,
}

fn models_state(state: &AppState) -> ModelsState {
    ModelsState { installed: models::installed(&state.models_dir), downloading: state.downloads.active_ids() }
}

#[tauri::command]
fn list_models(state: State<'_, AppState>) -> ModelsState {
    models_state(&state)
}

fn models_changed(app: &AppHandle) {
    let state = app.state::<AppState>();
    let _ = app.emit("models-changed", models_state(&state));
}

#[tauri::command]
fn download_model(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    let model = models::find(&id).ok_or("unknown model")?;
    let Some(cancel) = state.downloads.begin(&id) else { return Ok(()) };
    let dir = state.models_dir.clone();
    let downloads = state.downloads.clone();
    models_changed(&app);

    tauri::async_runtime::spawn(async move {
        let app2 = app.clone();
        let result = models::download(model, &dir, cancel, move |p| {
            let _ = app2.emit("download-progress", p);
        })
        .await;
        downloads.finish(model.id);
        if let Err(e) = result {
            if e != "cancelled" {
                let _ = app.emit(
                    "download-progress",
                    models::DownloadProgress {
                        id: model.id.into(),
                        downloaded: 0,
                        total: model.size,
                        bytes_per_sec: 0.0,
                        phase: "error",
                        error: Some(e),
                    },
                );
            }
        }
        models_changed(&app);
    });
    Ok(())
}

#[tauri::command]
fn cancel_download(state: State<'_, AppState>, id: String) {
    state.downloads.cancel(&id);
}

#[tauri::command]
fn delete_model(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    if state.busy.load(Ordering::Relaxed) {
        return Err(tr("Дождитесь окончания расшифровки.", "Wait until the transcription finishes."));
    }
    if let Some(path) = models::model_path(&state.models_dir, &id) {
        state.engine.lock().unwrap().unload();
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    if let Some(m) = models::find(&id) {
        let _ = std::fs::remove_file(state.models_dir.join(format!("{}.part", m.file)));
    }
    models_changed(&app);
    Ok(())
}

#[tauri::command]
fn import_model(app: AppHandle, state: State<'_, AppState>, path: String) -> Result<String, String> {
    let id = models::import(Path::new(&path), &state.models_dir)?;
    models_changed(&app);
    Ok(id)
}

#[tauri::command]
fn open_models_dir(state: State<'_, AppState>) -> Result<(), String> {
    std::fs::create_dir_all(&state.models_dir).map_err(|e| e.to_string())?;
    tauri_plugin_opener::open_path(&state.models_dir, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn reveal(path: String) -> Result<(), String> {
    tauri_plugin_opener::reveal_item_in_dir(path).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("only https links".into());
    }
    tauri_plugin_opener::open_url(url, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn copy_text(app: AppHandle, text: String) -> Result<(), String> {
    app.clipboard().write_text(text).map_err(|e| e.to_string())
}

#[tauri::command]
fn save_text(path: String, text: String) -> Result<(), String> {
    std::fs::write(&path, text).map_err(|e| e.to_string())
}

#[tauri::command]
fn transcribe(app: AppHandle, state: State<'_, AppState>, paths: Vec<String>) -> Result<(), String> {
    let sources = paths.into_iter().map(|p| Source::File(PathBuf::from(p))).collect::<Vec<_>>();
    start_job(app, &state, sources)
}

#[tauri::command]
fn cancel_job(state: State<'_, AppState>) {
    state.cancel.store(true, Ordering::Relaxed);
}

#[derive(Clone, Serialize)]
struct RecLevel {
    level: f32,
    seconds: f64,
}

#[tauri::command]
fn start_recording(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    if state.busy.load(Ordering::Relaxed) {
        return Err(tr("Дождитесь окончания расшифровки.", "Wait until the transcription finishes."));
    }
    let mut slot = state.recording.lock().unwrap();
    if slot.is_some() {
        return Ok(());
    }
    let rec = recorder::Recording::start(move |lvl| {
        let _ = app.emit("rec-level", RecLevel { level: lvl.level, seconds: lvl.seconds });
    })?;
    *slot = Some(rec);
    Ok(())
}

/// Останавливает запись и сразу отправляет её на расшифровку.
/// `base_name` — имя файлов для автосохранения, его собирает интерфейс
/// («Диктовка 2026-10-02 14-30-00»): там проще взять местное время.
#[tauri::command]
fn stop_recording(app: AppHandle, state: State<'_, AppState>, base_name: String) -> Result<(), String> {
    let rec = state.recording.lock().unwrap().take().ok_or("not recording")?;
    let samples = rec.stop()?;
    if samples.len() < audio::SAMPLE_RATE as usize {
        return Err(tr("Слишком короткая запись.", "The recording is too short."));
    }
    start_job(app, &state, vec![Source::Recording(samples, base_name)])
}

// MARK: - Очередь расшифровки

fn start_job(app: AppHandle, state: &AppState, sources: Vec<Source>) -> Result<(), String> {
    if sources.is_empty() {
        return Ok(());
    }
    if state.busy.swap(true, Ordering::SeqCst) {
        return Err(tr("Уже идёт расшифровка.", "A transcription is already running."));
    }
    state.cancel.store(false, Ordering::SeqCst);

    let settings = state.settings.lock().unwrap().clone();
    let model_path = settings.model.as_deref().and_then(|id| models::model_path(&state.models_dir, id));
    let engine = state.engine.clone();
    let busy = state.busy.clone();
    let cancel = state.cancel.clone();
    let documents = app.path().document_dir().ok();

    let spawned = std::thread::Builder::new().name("transcribe".into()).spawn(move || {
        let total = sources.len();
        for (index, source) in sources.into_iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            run_one(&app, &engine, &settings, model_path.as_deref(), documents.as_deref(), source, index, total);
        }
        busy.store(false, Ordering::SeqCst);
        emit_job(&app, JobEvent::Done { cancelled: cancel.load(Ordering::Relaxed) });
    });
    if let Err(e) = spawned {
        state.busy.store(false, Ordering::SeqCst);
        return Err(e.to_string());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_one(
    app: &AppHandle,
    engine: &Mutex<Engine>,
    settings: &Settings,
    model_path: Option<&Path>,
    documents: Option<&Path>,
    source: Source,
    index: usize,
    total: usize,
) {
    let name = match &source {
        Source::File(p) => p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        Source::Recording(..) => tr("Диктовка", "Dictation"),
    };
    let fail = |message: String, no_model: bool| {
        emit_job(app, JobEvent::Error { name: name.clone(), message, no_model });
    };
    emit_job(app, JobEvent::File { index, total, name: name.clone() });

    let Some(model_path) = model_path else {
        return fail(tr("Сначала выберите и скачайте модель.", "Choose and download a model first."), true);
    };

    // 1. Звук
    let is_rec = matches!(source, Source::Recording(..));
    emit_job(app, JobEvent::Phase { phase: "decoding", name: name.clone(), audio_seconds: 0.0, recording: is_rec });
    let (samples, source_path, rec_base) = match source {
        Source::File(path) => match audio::decode_file(&path) {
            Ok(s) => (s, Some(path), None),
            Err(e) => return fail(e, false),
        },
        Source::Recording(s, base) => (s, None, Some(base)),
    };
    let audio_seconds = samples.len() as f64 / audio::SAMPLE_RATE as f64;

    // 2. Модель (грузится один раз и остаётся в памяти)
    let mut engine = engine.lock().unwrap();
    if !engine.is_loaded_with(model_path, settings.use_gpu) {
        emit_job(app, JobEvent::Phase { phase: "loading", name: name.clone(), audio_seconds, recording: is_rec });
        if let Err(e) = engine.load(model_path, settings.use_gpu) {
            return fail(e.message(), false);
        }
    }

    // 3. Распознавание
    emit_job(app, JobEvent::Phase { phase: "transcribing", name: name.clone(), audio_seconds, recording: is_rec });
    let started = Instant::now();
    let mut last = Instant::now() - Duration::from_secs(1);
    let mut on_progress = |p: f64| {
        if last.elapsed() >= Duration::from_millis(150) || p >= 1.0 {
            emit_job(app, JobEvent::Progress { value: p.clamp(0.0, 1.0) });
            last = Instant::now();
        }
    };
    let language = (settings.language != "auto").then_some(settings.language.as_str());
    let segments: Vec<Segment> = match engine.transcribe(&samples, language, &mut on_progress) {
        Ok(s) => s,
        Err(EngineError::Cancelled) => return,
        Err(e) => return fail(e.message(), false),
    };
    let elapsed = started.elapsed().as_secs_f64();
    let text = if settings.timestamps { engine::timestamped_text(&segments) } else { engine::plain_text(&segments) };
    let language = engine.detected_language();
    let on_gpu = engine.on_gpu().unwrap_or(false);
    drop(engine);

    // 4. Автосохранение: файл → .txt рядом; диктовка → Документы/Whisper (+ .wav)
    let mut saved_to = None;
    if settings.auto_save && !text.is_empty() {
        if let Some(src) = &source_path {
            let out = src.with_extension("txt");
            if std::fs::write(&out, &text).is_ok() {
                saved_to = Some(out);
            }
        } else if let (Some(docs), Some(base)) = (documents, rec_base.as_deref()) {
            let dir = docs.join("Whisper");
            if std::fs::create_dir_all(&dir).is_ok() {
                let txt = dir.join(format!("{base}.txt"));
                if std::fs::write(&txt, &text).is_ok() {
                    let _ = audio::write_wav(&samples, &dir.join(format!("{base}.wav")));
                    saved_to = Some(txt);
                }
            }
        }
    }

    emit_job(
        app,
        JobEvent::Result {
            name,
            text,
            header: total > 1,
            elapsed,
            audio_seconds,
            language,
            saved_to: saved_to.map(|p| p.to_string_lossy().into()),
            on_gpu,
        },
    );
}

// MARK: - Файлы, открытые снаружи

fn open_paths(app: &AppHandle, paths: Vec<String>) {
    let paths: Vec<String> = paths.into_iter().filter(|p| Path::new(p).is_file()).collect();
    if paths.is_empty() {
        return;
    }
    let state = app.state::<AppState>();
    if state.ui_ready.load(Ordering::Relaxed) {
        let _ = app.emit("open-files", paths);
    } else {
        state.pending_open.lock().unwrap().extend(paths);
    }
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Аргументы командной строки, похожие на файлы (Windows/Linux «Открыть с помощью»).
fn file_args(args: impl IntoIterator<Item = String>) -> Vec<String> {
    args.into_iter().skip(1).filter(|a| !a.starts_with('-')).collect()
}

// MARK: - Запуск

pub fn run() {
    // whisper.cpp и ggml очень разговорчивы — глушим их лог в stderr.
    whisper_rs::install_logging_hooks();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            open_paths(app, file_args(argv));
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            let data_dir = app.path().app_data_dir()?;
            let settings_path = config_dir.join("settings.json");
            let settings = Settings::load(&settings_path);
            i18n::set_english(settings.english());

            let vad = app
                .path()
                .resolve(format!("resources/{VAD_FILE}"), tauri::path::BaseDirectory::Resource)
                .ok()
                .filter(|p| p.exists());

            let cancel = Arc::new(AtomicBool::new(false));
            let gpu_devices = engine::gpu_devices();
            let mut sys = sysinfo::System::new();
            sys.refresh_memory();
            let total_ram_gb = sys.total_memory() as f64 / 1024.0 / 1024.0 / 1024.0;
            let has_gpu = !gpu_devices.is_empty();

            let system = SystemInfo {
                os: std::env::consts::OS,
                arch: std::env::consts::ARCH,
                gpu_backend: if has_gpu { engine::gpu_backend_name() } else { "CPU" },
                gpu_devices,
                total_ram_gb,
                cores: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
                recommended: models::recommended(has_gpu, total_ram_gb),
                version: env!("CARGO_PKG_VERSION"),
                whisper_version: whisper_rs::WHISPER_CPP_VERSION,
            };

            app.manage(AppState {
                settings: Mutex::new(settings),
                settings_path,
                models_dir: data_dir.join("models"),
                engine: Arc::new(Mutex::new(Engine::new(vad, cancel.clone()))),
                cancel,
                busy: Arc::new(AtomicBool::new(false)),
                recording: Mutex::new(None),
                downloads: Arc::new(models::Downloads::default()),
                pending_open: Mutex::new(file_args(std::env::args())),
                ui_ready: AtomicBool::new(false),
                system,
            });

            // Полоса нагрузки: раз в секунду
            let handle = app.handle().clone();
            std::thread::Builder::new().name("monitor".into()).spawn(move || {
                let mut mon = monitor::Monitor::new();
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                    let _ = handle.emit("stats", mon.sample());
                }
            })?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            save_settings,
            list_models,
            download_model,
            cancel_download,
            delete_model,
            import_model,
            open_models_dir,
            reveal,
            open_url,
            copy_text,
            save_text,
            transcribe,
            cancel_job,
            start_recording,
            stop_recording,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Auris Whisper");

    app.run(|_app, _event| {
        // macOS: файлы, брошенные на иконку или открытые через Finder.
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Opened { urls } = _event {
            let paths = urls
                .into_iter()
                .filter_map(|u| u.to_file_path().ok())
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            open_paths(_app, paths);
        }
    });
}
