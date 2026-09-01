import Foundation
import IOKit

/// Живые показатели нагрузки: сколько ест само приложение и что происходит с железом.
final class SystemMonitor: ObservableObject {

    struct Stats {
        var appCPU: Double = 0          // % (100 % = одно ядро целиком)
        var systemCPU: Double = 0       // % от всех ядер
        var gpu: Double = 0             // % загрузки видеоядра
        var memoryMB: Double = 0        // память приложения (как в «Мониторинге системы»)
        var hasGPU: Bool = false
    }

    @Published private(set) var stats = Stats()

    let coreCount = ProcessInfo.processInfo.activeProcessorCount
    private var timer: Timer?
    private var lastAppTime: Double = 0
    private var lastWallTime: Double = 0
    private var lastTicks: (user: UInt32, system: UInt32, idle: UInt32, nice: UInt32)?

    func start() {
        guard timer == nil else { return }
        sample()
        let t = Timer(timeInterval: 1.0, repeats: true) { [weak self] _ in self?.sample() }
        RunLoop.main.add(t, forMode: .common)
        timer = t
    }

    func stop() { timer?.invalidate(); timer = nil }

    private func sample() {
        var s = Stats()
        let now = CFAbsoluteTimeGetCurrent()

        // Процессорное время самого приложения
        let cpuTime = Self.processCPUSeconds()
        if lastWallTime > 0, now > lastWallTime {
            s.appCPU = max(0, (cpuTime - lastAppTime) / (now - lastWallTime) * 100.0)
        }
        lastAppTime = cpuTime
        lastWallTime = now

        // Загрузка всех ядер системы
        if let ticks = Self.hostTicks() {
            if let prev = lastTicks {
                let user   = Double(ticks.user &- prev.user)
                let system = Double(ticks.system &- prev.system)
                let idle   = Double(ticks.idle &- prev.idle)
                let nice   = Double(ticks.nice &- prev.nice)
                let total  = user + system + idle + nice
                if total > 0 { s.systemCPU = (user + system + nice) / total * 100.0 }
            }
            lastTicks = ticks
        }

        s.memoryMB = Self.memoryFootprintMB()
        if let g = Self.gpuUtilization() { s.gpu = g; s.hasGPU = true }

        stats = s
    }

    // MARK: - mach / IOKit

    /// Суммарное процессорное время процесса в секундах (живые + завершённые потоки).
    private static func processCPUSeconds() -> Double {
        var seconds: Double = 0

        var basic = task_basic_info()
        var basicCount = mach_msg_type_number_t(MemoryLayout<task_basic_info>.size / MemoryLayout<natural_t>.size)
        let krBasic = withUnsafeMutablePointer(to: &basic) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(basicCount)) {
                task_info(mach_task_self_, task_flavor_t(TASK_BASIC_INFO), $0, &basicCount)
            }
        }
        if krBasic == KERN_SUCCESS {
            seconds += Double(basic.user_time.seconds) + Double(basic.user_time.microseconds) / 1e6
            seconds += Double(basic.system_time.seconds) + Double(basic.system_time.microseconds) / 1e6
        }

        var times = task_thread_times_info()
        var timesCount = mach_msg_type_number_t(MemoryLayout<task_thread_times_info>.size / MemoryLayout<natural_t>.size)
        let krTimes = withUnsafeMutablePointer(to: &times) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(timesCount)) {
                task_info(mach_task_self_, task_flavor_t(TASK_THREAD_TIMES_INFO), $0, &timesCount)
            }
        }
        if krTimes == KERN_SUCCESS {
            seconds += Double(times.user_time.seconds) + Double(times.user_time.microseconds) / 1e6
            seconds += Double(times.system_time.seconds) + Double(times.system_time.microseconds) / 1e6
        }
        return seconds
    }

    private static func hostTicks() -> (user: UInt32, system: UInt32, idle: UInt32, nice: UInt32)? {
        var info = host_cpu_load_info()
        var count = mach_msg_type_number_t(MemoryLayout<host_cpu_load_info>.size / MemoryLayout<integer_t>.size)
        let kr = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                host_statistics(mach_host_self(), HOST_CPU_LOAD_INFO, $0, &count)
            }
        }
        guard kr == KERN_SUCCESS else { return nil }
        return (info.cpu_ticks.0, info.cpu_ticks.1, info.cpu_ticks.2, info.cpu_ticks.3)
    }

    private static func memoryFootprintMB() -> Double {
        var info = task_vm_info_data_t()
        var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<integer_t>.size)
        let kr = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
            }
        }
        guard kr == KERN_SUCCESS else { return 0 }
        return Double(info.phys_footprint) / 1024 / 1024
    }

    /// Загрузка видеоядра из IORegistry (та же цифра, что показывает «Мониторинг системы»).
    private static func gpuUtilization() -> Double? {
        var iterator: io_iterator_t = 0
        guard IOServiceGetMatchingServices(kIOMainPortDefault,
                                           IOServiceMatching("IOAccelerator"),
                                           &iterator) == KERN_SUCCESS else { return nil }
        defer { IOObjectRelease(iterator) }

        var result: Double?
        var service = IOIteratorNext(iterator)
        while service != 0 {
            var props: Unmanaged<CFMutableDictionary>?
            if IORegistryEntryCreateCFProperties(service, &props, kCFAllocatorDefault, 0) == KERN_SUCCESS,
               let dict = props?.takeRetainedValue() as? [String: Any],
               let stats = dict["PerformanceStatistics"] as? [String: Any] {
                if let v = stats["Device Utilization %"] as? Int { result = Double(v) }
                else if let v = stats["Renderer Utilization %"] as? Int { result = Double(v) }
            }
            IOObjectRelease(service)
            if result != nil { break }
            service = IOIteratorNext(iterator)
        }
        return result
    }
}
