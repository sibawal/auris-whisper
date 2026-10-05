//! Запись с микрофона сразу в 16 кГц моно f32 — ровно то, что ест whisper.
//!
//! Поток cpal живёт в своём системном потоке (на macOS он не `Send`).
//! Колбэк звуковой карты только складывает моно-отсчёты в общий буфер,
//! а пересэмплирование идёт в рабочем потоке раз в 100 мс.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use serde::Serialize;

use crate::audio::{StreamResampler, SAMPLE_RATE};
use crate::i18n::tr;

/// Тише этого за первые секунды записи — считаем, что микрофон молчит:
/// система отдаёт нули (нет доступа) или выбран не тот вход.
const SILENCE_PEAK: f32 = 1e-4;
const SILENCE_AFTER: Duration = Duration::from_millis(1500);

pub struct Recording {
    stop_tx: mpsc::Sender<()>,
    worker: Option<JoinHandle<Result<Vec<f32>, String>>>,
}

pub struct Level {
    pub level: f32,
    pub seconds: f64,
    /// За первые полторы секунды не пришло ни одного заметного отсчёта.
    pub silent: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InputDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// Все микрофоны, которые видит система. Первым идёт микрофон по умолчанию.
pub fn list_devices() -> Vec<InputDevice> {
    let host = cpal::default_host();
    let default_id = host.default_input_device().and_then(|d| d.id().ok()).map(|id| id.to_string());
    let mut out: Vec<InputDevice> = host
        .input_devices()
        .map(|devs| {
            devs.filter_map(|d| {
                let id = d.id().ok()?.to_string();
                Some(InputDevice { is_default: Some(&id) == default_id.as_ref(), name: d.to_string(), id })
            })
            .collect()
        })
        .unwrap_or_default();
    out.sort_by_key(|d| !d.is_default);
    out
}

impl Recording {
    /// Запускает запись с выбранного микрофона (или системного по умолчанию).
    /// `on_level` зовётся примерно 12 раз в секунду.
    pub fn start(device_id: Option<String>, on_level: impl Fn(Level) + Send + 'static) -> Result<Self, String> {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();

        let worker = std::thread::Builder::new()
            .name("recorder".into())
            .spawn(move || run(device_id, stop_rx, ready_tx, on_level))
            .map_err(|e| e.to_string())?;

        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => Ok(Self { stop_tx, worker: Some(worker) }),
            Ok(Err(e)) => {
                let _ = worker.join();
                Err(e)
            }
            Err(_) => Err(tr("Микрофон не отвечает.", "The microphone does not respond.")),
        }
    }

    /// Останавливает запись и отдаёт всё записанное (16 кГц моно).
    pub fn stop(mut self) -> Result<Vec<f32>, String> {
        let _ = self.stop_tx.send(());
        match self.worker.take().map(|w| w.join()) {
            Some(Ok(result)) => result,
            _ => Err(tr("Запись прервалась.", "The recording was interrupted.")),
        }
    }
}

/// Префикс ошибок про микрофон: по нему интерфейс показывает кнопку
/// «Открыть настройки микрофона».
pub const MIC_ERROR: &str = "mic:";

fn mic_unavailable(detail: Option<String>) -> String {
    let mut msg = String::from(MIC_ERROR);
    msg += &tr(
        "Микрофон недоступен. Проверьте, что он подключён и что приложению разрешён доступ к нему в настройках конфиденциальности.",
        "The microphone is unavailable. Make sure it is connected and the app is allowed to use it in the privacy settings.",
    );
    if let Some(d) = detail {
        msg += &format!(" ({d})");
    }
    msg
}

fn mic_silent() -> String {
    String::from(MIC_ERROR)
        + &tr(
            "Микрофон ничего не передал — запись получилась пустой. Чаще всего система не даёт приложению доступ к микрофону или выбран не тот микрофон.",
            "The microphone sent nothing — the recording is empty. Usually the system denies the app microphone access, or the wrong microphone is selected.",
        )
}

fn pick_device(host: &cpal::Host, id: Option<&str>) -> Option<cpal::Device> {
    if let Some(id) = id {
        if let Ok(parsed) = id.parse::<cpal::DeviceId>() {
            if let Some(d) = host.device_by_id(&parsed) {
                return Some(d);
            }
        }
    }
    host.default_input_device()
}

fn run(
    device_id: Option<String>,
    stop_rx: mpsc::Receiver<()>,
    ready_tx: mpsc::Sender<Result<(), String>>,
    on_level: impl Fn(Level),
) -> Result<Vec<f32>, String> {
    let fail = |msg: String| {
        let _ = ready_tx.send(Err(msg.clone()));
        Err(msg)
    };

    let host = cpal::default_host();
    let Some(device) = pick_device(&host, device_id.as_deref()) else {
        return fail(mic_unavailable(Some(tr("в системе нет ни одного микрофона", "no microphone found"))));
    };
    let supported = match device.default_input_config() {
        Ok(c) => c,
        Err(e) => return fail(mic_unavailable(Some(e.to_string()))),
    };
    let config = supported.config();
    let channels = config.channels.max(1) as usize;
    let rate = config.sample_rate;

    let raw: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::with_capacity(rate as usize)));
    let peak = Arc::new(AtomicU32::new(0));
    let failed: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let any_failed = Arc::new(AtomicBool::new(false));

    let stream = {
        let failed = failed.clone();
        let any_failed = any_failed.clone();
        let err_cb = move |e: cpal::Error| {
            any_failed.store(true, Ordering::Relaxed);
            *failed.lock().unwrap() = Some(e.to_string());
        };
        let build = |fmt: SampleFormat| -> Result<cpal::Stream, cpal::Error> {
            match fmt {
                SampleFormat::F32 => build::<f32>(&device, config, channels, raw.clone(), peak.clone(), err_cb.clone()),
                SampleFormat::I16 => build::<i16>(&device, config, channels, raw.clone(), peak.clone(), err_cb.clone()),
                SampleFormat::I32 => build::<i32>(&device, config, channels, raw.clone(), peak.clone(), err_cb.clone()),
                SampleFormat::U16 => build::<u16>(&device, config, channels, raw.clone(), peak.clone(), err_cb.clone()),
                SampleFormat::U8 => build::<u8>(&device, config, channels, raw.clone(), peak.clone(), err_cb.clone()),
                SampleFormat::I8 => build::<i8>(&device, config, channels, raw.clone(), peak.clone(), err_cb.clone()),
                SampleFormat::F64 => build::<f64>(&device, config, channels, raw.clone(), peak.clone(), err_cb.clone()),
                _ => build::<f32>(&device, config, channels, raw.clone(), peak.clone(), err_cb.clone()),
            }
        };
        match build(supported.sample_format()) {
            Ok(s) => s,
            Err(e) => return fail(mic_unavailable(Some(e.to_string()))),
        }
    };
    if let Err(e) = stream.play() {
        return fail(mic_unavailable(Some(e.to_string())));
    }
    let _ = ready_tx.send(Ok(()));

    let started = Instant::now();
    let mut resampler = StreamResampler::new(rate)?;
    let mut out: Vec<f32> = Vec::with_capacity(SAMPLE_RATE as usize * 60);
    let mut pending = Vec::new();

    loop {
        let stop = match stop_rx.recv_timeout(Duration::from_millis(80)) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => true,
            Err(mpsc::RecvTimeoutError::Timeout) => false,
        };

        {
            let mut buf = raw.lock().unwrap();
            std::mem::swap(&mut *buf, &mut pending);
        }
        let mut rms = 0.0f32;
        if !pending.is_empty() {
            let sum: f32 = pending.iter().map(|v| v * v).sum();
            rms = (sum / pending.len() as f32).sqrt();
            resampler.push(&pending, &mut out);
            pending.clear();
        }
        let loudest = f32::from_bits(peak.load(Ordering::Relaxed));
        on_level(Level {
            level: (rms * 12.0).min(1.0),
            seconds: out.len() as f64 / SAMPLE_RATE as f64,
            silent: started.elapsed() >= SILENCE_AFTER && loudest < SILENCE_PEAK,
        });

        if stop || any_failed.load(Ordering::Relaxed) {
            break;
        }
    }
    drop(stream);
    out.extend(resampler.finish());

    if let Some(e) = failed.lock().unwrap().take() {
        if out.len() < SAMPLE_RATE as usize {
            return Err(mic_unavailable(Some(e)));
        }
    }
    // Система молча отдаёт нули, если доступ к микрофону запрещён.
    if f32::from_bits(peak.load(Ordering::Relaxed)) < SILENCE_PEAK && out.len() > SAMPLE_RATE as usize / 2 {
        return Err(mic_silent());
    }
    Ok(out)
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    raw: Arc<Mutex<Vec<f32>>>,
    peak: Arc<AtomicU32>,
    err_cb: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample + cpal::Sample,
    f32: cpal::FromSample<T>,
{
    let mut mixed: Vec<f32> = Vec::new();
    device.build_input_stream::<T, _, _>(
        config,
        move |data: &[T], _| {
            // Сводим каналы в моно. Бывают микрофоны с противофазными каналами:
            // среднее у них гасит само себя — тогда берём первый канал.
            mixed.clear();
            let (mut e_mix, mut e_first, mut local_peak) = (0.0f32, 0.0f32, 0.0f32);
            for frame in data.chunks(channels) {
                let first = f32::from_sample_(frame[0]);
                let s = frame.iter().map(|v| f32::from_sample_(*v)).sum::<f32>() / frame.len() as f32;
                e_mix += s * s;
                e_first += first * first;
                local_peak = local_peak.max(first.abs()).max(s.abs());
                mixed.push(s);
            }
            if channels > 1 && e_first > 0.0 && e_mix < e_first * 0.05 {
                mixed.clear();
                mixed.extend(data.chunks(channels).map(|f| f32::from_sample_(f[0])));
            }
            raw.lock().unwrap().extend_from_slice(&mixed);
            let prev = f32::from_bits(peak.load(Ordering::Relaxed));
            if local_peak > prev {
                peak.store(local_peak.to_bits(), Ordering::Relaxed);
            }
        },
        err_cb,
        None,
    )
}

// MARK: - Разрешение на микрофон

/// Запрещён ли доступ к микрофону на уровне системы. На Windows обычные
/// программы не получают запроса, как на macOS: доступ включается переключателем
/// «Разрешить классическим приложениям доступ к микрофону». Если он выключен,
/// Windows молча отдаёт тишину — поэтому проверяем его заранее.
#[cfg(target_os = "windows")]
pub fn access_denied() -> bool {
    use windows::core::{w, PCWSTR};
    use windows::Win32::System::Registry::{RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};

    fn read(root: HKEY, key: PCWSTR) -> Option<String> {
        let mut buf = [0u16; 32];
        let mut size = (buf.len() * 2) as u32;
        let rc = unsafe {
            RegGetValueW(root, key, w!("Value"), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut size))
        };
        if rc.is_err() {
            return None;
        }
        let len = (size as usize / 2).saturating_sub(1).min(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }

    // «Доступ к микрофону» для всего устройства и «…классическим приложениям» для пользователя.
    let device = w!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\microphone");
    let desktop =
        w!("Software\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\microphone\\NonPackaged");
    read(HKEY_LOCAL_MACHINE, device).as_deref() == Some("Deny")
        || read(HKEY_CURRENT_USER, desktop).as_deref() == Some("Deny")
}

#[cfg(not(target_os = "windows"))]
pub fn access_denied() -> bool {
    false
}

/// Открыть системные настройки доступа к микрофону.
pub fn open_settings() -> Result<(), String> {
    let url = if cfg!(target_os = "windows") {
        "ms-settings:privacy-microphone"
    } else if cfg!(target_os = "macos") {
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
    } else {
        return Err(tr(
            "Откройте настройки звука вашей системы и проверьте, какой микрофон выбран.",
            "Open your system sound settings and check which microphone is selected.",
        ));
    };
    tauri_plugin_opener::open_url(url, None::<&str>).map_err(|e| e.to_string())
}
