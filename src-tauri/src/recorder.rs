//! Запись с микрофона сразу в 16 кГц моно f32 — ровно то, что ест whisper.
//!
//! Поток cpal живёт в своём системном потоке (на macOS он не `Send`).
//! Колбэк звуковой карты только складывает моно-отсчёты в общий буфер,
//! а пересэмплирование идёт в рабочем потоке раз в 100 мс.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

use crate::audio::{StreamResampler, SAMPLE_RATE};
use crate::i18n::tr;

pub struct Recording {
    stop_tx: mpsc::Sender<()>,
    worker: Option<JoinHandle<Result<Vec<f32>, String>>>,
}

pub struct Level {
    pub level: f32,
    pub seconds: f64,
}

impl Recording {
    /// Запускает запись. `on_level` зовётся примерно 12 раз в секунду.
    pub fn start(on_level: impl Fn(Level) + Send + 'static) -> Result<Self, String> {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();

        let worker = std::thread::Builder::new()
            .name("recorder".into())
            .spawn(move || run(stop_rx, ready_tx, on_level))
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

fn mic_unavailable() -> String {
    tr(
        "Микрофон недоступен. Проверьте, что он подключён и что приложению разрешён доступ к нему в настройках конфиденциальности.",
        "The microphone is unavailable. Make sure it is connected and the app is allowed to use it in the privacy settings.",
    )
}

fn run(
    stop_rx: mpsc::Receiver<()>,
    ready_tx: mpsc::Sender<Result<(), String>>,
    on_level: impl Fn(Level),
) -> Result<Vec<f32>, String> {
    let fail = |msg: String| {
        let _ = ready_tx.send(Err(msg.clone()));
        Err(msg)
    };

    let host = cpal::default_host();
    let Some(device) = host.default_input_device() else { return fail(mic_unavailable()) };
    let supported = match device.default_input_config() {
        Ok(c) => c,
        Err(_) => return fail(mic_unavailable()),
    };
    let config = supported.config();
    let channels = config.channels.max(1) as usize;
    let rate = config.sample_rate;

    let raw: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::with_capacity(rate as usize)));
    let peak = Arc::new(AtomicU32::new(0));
    let failed = Arc::new(AtomicBool::new(false));

    let stream = {
        let failed = failed.clone();
        let err_cb = move |_e: cpal::Error| failed.store(true, Ordering::Relaxed);
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
            Err(_) => return fail(mic_unavailable()),
        }
    };
    if stream.play().is_err() {
        return fail(mic_unavailable());
    }
    let _ = ready_tx.send(Ok(()));

    let mut resampler = StreamResampler::new(rate)?;
    let mut out: Vec<f32> = Vec::with_capacity(SAMPLE_RATE as usize * 60);
    let mut loudest = 0.0f32;
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
        loudest = loudest.max(f32::from_bits(peak.load(Ordering::Relaxed)));
        on_level(Level { level: (rms * 12.0).min(1.0), seconds: out.len() as f64 / SAMPLE_RATE as f64 });

        if stop || failed.load(Ordering::Relaxed) {
            break;
        }
    }
    drop(stream);
    out.extend(resampler.finish());

    if failed.load(Ordering::Relaxed) && out.is_empty() {
        return Err(mic_unavailable());
    }
    // Система молча отдаёт нули, если доступ к микрофону запрещён.
    if loudest == 0.0 && out.len() > SAMPLE_RATE as usize / 2 {
        return Err(mic_unavailable());
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
    device.build_input_stream::<T, _, _>(
        config,
        move |data: &[T], _| {
            let mut local_peak = 0.0f32;
            let mut buf = raw.lock().unwrap();
            for frame in data.chunks(channels) {
                let s: f32 = frame.iter().map(|v| f32::from_sample_(*v)).sum::<f32>() / channels as f32;
                local_peak = local_peak.max(s.abs());
                buf.push(s);
            }
            drop(buf);
            let prev = f32::from_bits(peak.load(Ordering::Relaxed));
            if local_peak > prev {
                peak.store(local_peak.to_bits(), Ordering::Relaxed);
            }
        },
        err_cb,
        None,
    )
}
