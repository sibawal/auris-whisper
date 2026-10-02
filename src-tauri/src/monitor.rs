//! Живые показатели нагрузки: сколько ест само приложение и что происходит с железом.

use serde::Serialize;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System, CpuRefreshKind};

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    /// % (100 % = одно ядро целиком)
    pub app_cpu: f64,
    /// % от всех ядер
    pub system_cpu: f64,
    /// % загрузки видеоядра; `None`, если система не говорит
    pub gpu: Option<f64>,
    /// память приложения, МБ
    pub memory_mb: f64,
    pub cores: usize,
}

pub struct Monitor {
    sys: System,
    pid: Option<Pid>,
    gpu: gpu::Probe,
}

impl Default for Monitor {
    fn default() -> Self {
        Self::new()
    }
}

impl Monitor {
    pub fn new() -> Self {
        let sys = System::new_with_specifics(RefreshKind::nothing().with_cpu(CpuRefreshKind::nothing().with_cpu_usage()));
        Self { sys, pid: sysinfo::get_current_pid().ok(), gpu: gpu::Probe::new() }
    }

    /// Зовётся раз в секунду: проценты считаются по разнице с прошлым замером.
    pub fn sample(&mut self) -> Stats {
        self.sys.refresh_cpu_usage();
        let mut s = Stats { cores: self.sys.cpus().len().max(1), ..Default::default() };
        s.system_cpu = self.sys.global_cpu_usage() as f64;

        if let Some(pid) = self.pid {
            self.sys.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[pid]),
                false,
                ProcessRefreshKind::nothing().with_cpu().with_memory(),
            );
            if let Some(p) = self.sys.process(pid) {
                s.app_cpu = p.cpu_usage() as f64;
                s.memory_mb = p.memory() as f64 / 1024.0 / 1024.0;
            }
        }
        if let Some(fp) = footprint_mb() {
            s.memory_mb = fp;
        }
        s.gpu = self.gpu.utilization();
        s
    }
}

/// На macOS берём phys_footprint — ту же цифру, что «Мониторинг системы».
/// Резидентный объём там не учитывает память, отданную видеоядру.
#[cfg(target_os = "macos")]
fn footprint_mb() -> Option<f64> {
    let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        libc::proc_pid_rusage(
            std::process::id() as i32,
            libc::RUSAGE_INFO_V2,
            &mut info as *mut _ as *mut libc::rusage_info_t,
        )
    };
    (rc == 0).then(|| info.ri_phys_footprint as f64 / 1024.0 / 1024.0)
}

#[cfg(not(target_os = "macos"))]
fn footprint_mb() -> Option<f64> {
    None
}

// MARK: - Видеоядро

#[cfg(target_os = "macos")]
mod gpu {
    //! Загрузка видеоядра из IORegistry (та же цифра, что показывает «Мониторинг системы»).
    use core_foundation_sys::base::{kCFAllocatorDefault, CFRelease};
    use core_foundation_sys::dictionary::{CFDictionaryGetValue, CFDictionaryRef, CFMutableDictionaryRef};
    use core_foundation_sys::number::{kCFNumberSInt64Type, CFNumberGetValue, CFNumberRef};
    use core_foundation_sys::string::{kCFStringEncodingUTF8, CFStringCreateWithCString, CFStringRef};
    use io_kit_sys::types::io_iterator_t;
    use io_kit_sys::*;
    use std::ffi::{c_void, CString};

    pub struct Probe;

    impl Probe {
        pub fn new() -> Self {
            Probe
        }

        pub fn utilization(&mut self) -> Option<f64> {
            unsafe { read() }
        }
    }

    unsafe fn cfstr(s: &str) -> CFStringRef {
        let c = CString::new(s).unwrap();
        CFStringCreateWithCString(kCFAllocatorDefault, c.as_ptr(), kCFStringEncodingUTF8)
    }

    unsafe fn read() -> Option<f64> {
        let matching = IOServiceMatching(c"IOAccelerator".as_ptr());
        let mut iter: io_iterator_t = 0;
        if IOServiceGetMatchingServices(kIOMasterPortDefault, matching, &mut iter) != 0 {
            return None;
        }
        let k_stats = cfstr("PerformanceStatistics");
        let k_dev = cfstr("Device Utilization %");
        let k_ren = cfstr("Renderer Utilization %");
        let mut result = None;
        loop {
            let service = IOIteratorNext(iter);
            if service == 0 {
                break;
            }
            let mut props: CFMutableDictionaryRef = std::ptr::null_mut();
            if IORegistryEntryCreateCFProperties(service, &mut props, kCFAllocatorDefault, 0) == 0 && !props.is_null() {
                let stats = CFDictionaryGetValue(props as CFDictionaryRef, k_stats as *const c_void);
                if !stats.is_null() {
                    for key in [k_dev, k_ren] {
                        let num = CFDictionaryGetValue(stats as CFDictionaryRef, key as *const c_void);
                        if !num.is_null() {
                            let mut v: i64 = 0;
                            if CFNumberGetValue(num as CFNumberRef, kCFNumberSInt64Type, &mut v as *mut i64 as *mut c_void) {
                                result = Some(v as f64);
                                break;
                            }
                        }
                    }
                }
                CFRelease(props as *const c_void);
            }
            IOObjectRelease(service);
            if result.is_some() {
                break;
            }
        }
        IOObjectRelease(iter);
        CFRelease(k_stats as *const c_void);
        CFRelease(k_dev as *const c_void);
        CFRelease(k_ren as *const c_void);
        result
    }
}

#[cfg(target_os = "windows")]
mod gpu {
    //! Счётчики производительности Windows: «\GPU Engine(*)\Utilization Percentage» —
    //! то же, что показывает «Диспетчер задач». Складываем загрузку каждого движка
    //! по всем процессам и берём самый загруженный.
    use std::collections::HashMap;
    use windows::core::w;
    use windows::Win32::System::Performance::{
        PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
        PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
    };

    pub struct Probe {
        query: PDH_HQUERY,
        counter: PDH_HCOUNTER,
        ok: bool,
    }

    // Дескрипторы PDH используются только из потока монитора.
    unsafe impl Send for Probe {}

    impl Probe {
        pub fn new() -> Self {
            let mut query = PDH_HQUERY::default();
            let mut counter = PDH_HCOUNTER::default();
            let ok = unsafe {
                PdhOpenQueryW(None, 0, &mut query) == 0
                    && PdhAddEnglishCounterW(query, w!("\\GPU Engine(*)\\Utilization Percentage"), 0, &mut counter) == 0
                    && PdhCollectQueryData(query) == 0
            };
            Self { query, counter, ok }
        }

        pub fn utilization(&mut self) -> Option<f64> {
            if !self.ok {
                return None;
            }
            unsafe {
                if PdhCollectQueryData(self.query) != 0 {
                    return None;
                }
                let mut size = 0u32;
                let mut count = 0u32;
                let rc = PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut size, &mut count, None);
                if rc != PDH_MORE_DATA || size == 0 {
                    return Some(0.0);
                }
                let mut buf = vec![0u8; size as usize];
                let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
                if PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut size, &mut count, Some(items)) != 0 {
                    return None;
                }
                let mut per_engine: HashMap<String, f64> = HashMap::new();
                for i in 0..count as usize {
                    let item = &*items.add(i);
                    let name = item.szName.to_string().unwrap_or_default();
                    // pid_1234_luid_0x..._0x..._phys_0_eng_3_engtype_Compute → ключ движка без pid
                    let key = name.split_once("_luid_").map(|(_, rest)| rest.to_string()).unwrap_or(name);
                    *per_engine.entry(key).or_default() += item.FmtValue.Anonymous.doubleValue;
                }
                Some(per_engine.values().cloned().fold(0.0, f64::max).min(100.0))
            }
        }
    }

    impl Drop for Probe {
        fn drop(&mut self) {
            unsafe {
                let _ = PdhCloseQuery(self.query);
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod gpu {
    //! AMD и Intel (i915/xe) отдают загрузку в sysfs. NVIDIA — нет, тогда полоску прячем.
    pub struct Probe {
        path: Option<std::path::PathBuf>,
    }

    impl Probe {
        pub fn new() -> Self {
            let path = std::fs::read_dir("/sys/class/drm").ok().and_then(|rd| {
                rd.flatten()
                    .map(|e| e.path().join("device/gpu_busy_percent"))
                    .find(|p| p.exists())
            });
            Self { path }
        }

        pub fn utilization(&mut self) -> Option<f64> {
            let p = self.path.as_ref()?;
            std::fs::read_to_string(p).ok()?.trim().parse().ok()
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod gpu {
    pub struct Probe;
    impl Probe {
        pub fn new() -> Self {
            Probe
        }
        pub fn utilization(&mut self) -> Option<f64> {
            None
        }
    }
}
