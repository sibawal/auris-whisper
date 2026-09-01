import Foundation

struct Segment: Identifiable {
    let id = UUID()
    let start: Double      // секунды
    let end: Double
    let text: String
}

enum EngineError: LocalizedError {
    case modelMissing
    case modelLoadFailed
    case runFailed(Int32)
    case cancelled

    var errorDescription: String? {
        switch self {
        case .modelMissing:      return tr("Модель не найдена внутри приложения.", "The model is missing from the app bundle.")
        case .modelLoadFailed:   return tr("Не удалось загрузить модель.", "Could not load the model.")
        case .runFailed(let c):  return tr("Ошибка распознавания (код \(c)).", "Transcription failed (code \(c)).")
        case .cancelled:         return tr("Отменено.", "Cancelled.")
        }
    }
}

/// Обёртка над whisper.cpp. Контекст модели живёт, пока живёт объект,
/// поэтому вторая и последующие расшифровки стартуют мгновенно.
final class WhisperEngine {

    private var ctx: OpaquePointer?
    private let lock = NSLock()

    private var progressHandler: ((Double) -> Void)?
    private var cancelRequested = false

    /// Единственный .bin в Resources — какую модель положили при сборке, ту и берём.
    static func bundledModelPath() -> String? {
        (Bundle.main.urls(forResourcesWithExtension: "bin", subdirectory: nil) ?? [])
            .first { !$0.lastPathComponent.hasPrefix("ggml-silero") }?.path
    }

    /// Короткое имя модели для интерфейса, например «large-v3».
    static var modelDisplayName: String {
        guard let path = bundledModelPath() else { return "—" }
        return URL(fileURLWithPath: path).deletingPathExtension().lastPathComponent
            .replacingOccurrences(of: "ggml-", with: "")
    }

    /// Путь к модели. По умолчанию — та, что лежит внутри бандла.
    private let overridePath: String?

    init(modelPath: String? = nil) { self.overridePath = modelPath }

    static let vadModelName = "ggml-silero-v5.1.2"

    /// Модель детектора речи: в бандле или рядом с основной моделью.
    private func vadModelPath() -> String? {
        if let p = Bundle.main.path(forResource: Self.vadModelName, ofType: "bin") { return p }
        if let base = overridePath {
            let side = URL(fileURLWithPath: base).deletingLastPathComponent()
                .appendingPathComponent(Self.vadModelName + ".bin")
            if FileManager.default.fileExists(atPath: side.path) { return side.path }
        }
        return nil
    }

    var isLoaded: Bool { ctx != nil }

    deinit { if let ctx { whisper_free(ctx) } }

    // MARK: - Загрузка модели

    func loadIfNeeded() throws {
        lock.lock(); defer { lock.unlock() }
        if ctx != nil { return }

        guard let path = overridePath ?? Self.bundledModelPath() else {
            throw EngineError.modelMissing
        }

        var cparams = whisper_context_default_params()
        cparams.use_gpu    = true      // Metal на Apple Silicon
        cparams.flash_attn = true

        guard let c = whisper_init_from_file_with_params(path, cparams) else {
            throw EngineError.modelLoadFailed
        }
        ctx = c
    }

    func cancel() { cancelRequested = true }

    // MARK: - Распознавание

    /// Публичный вход: режет длинное аудио на куски и склеивает результат.
    ///
    /// Whisper иногда застревает: окно перестаёт двигаться вперёд, и модель до конца
    /// файла повторяет одну фразу. Внутри одного вызова из этого не выбраться, поэтому
    /// длинную запись обрабатываем частями — срыв портит максимум один кусок,
    /// а следующий стартует с чистого листа.
    func transcribe(samples: [Float],
                    language: String?,
                    onProgress: @escaping (Double) -> Void) throws -> [Segment] {

        let rate = Int(WHISPER_SAMPLE_RATE)
        let chunkTarget = 5 * 60 * rate          // куски примерно по 5 минут
        guard samples.count > chunkTarget + 60 * rate else {
            return try transcribeChunk(samples: samples, language: language, onProgress: onProgress)
        }

        let bounds = Self.chunkBounds(samples, target: chunkTarget, rate: rate)
        var out: [Segment] = []
        var derailed = 0

        for (index, range) in bounds.enumerated() {
            let offset = Double(range.lowerBound) / Double(rate)
            let piece = Array(samples[range])
            let base = Double(index) / Double(bounds.count)
            let span = 1.0 / Double(bounds.count)

            var segments = try transcribeChunk(samples: piece, language: language) { p in
                onProgress(base + p * span)
            }

            // Кусок сорвался в повтор — пробуем ещё раз, с другой температурой.
            if Self.longestRepeatRun(segments) >= 5 {
                derailed += 1
                let retry = try? transcribeChunk(samples: piece, language: language,
                                                 temperature: 0.4) { p in onProgress(base + p * span) }
                if let retry, Self.longestRepeatRun(retry) < Self.longestRepeatRun(segments) {
                    segments = retry
                }
            }

            out.append(contentsOf: segments.map {
                Segment(start: $0.start + offset, end: $0.end + offset, text: $0.text)
            })
        }
        lastDerailedChunks = derailed
        return Self.collapseRepeats(out)
    }

    /// Сколько кусков сорвалось в повтор в последнем прогоне (для строки состояния).
    private(set) var lastDerailedChunks = 0

    /// Границы кусков: режем по самому тихому месту рядом с целевой точкой,
    /// чтобы не разорвать слово.
    static func chunkBounds(_ samples: [Float], target: Int, rate: Int) -> [Range<Int>] {
        var bounds: [Range<Int>] = []
        var start = 0
        let search = 15 * rate                  // ищем тишину в ±15 с от точки реза
        let frame = rate / 10                   // окно 100 мс

        while start < samples.count {
            let nominal = start + target
            if nominal >= samples.count - 30 * rate {
                bounds.append(start..<samples.count)
                break
            }
            var bestCut = nominal
            var bestEnergy = Float.greatestFiniteMagnitude
            var i = max(start + rate, nominal - search)
            let upper = min(samples.count - frame, nominal + search)
            while i < upper {
                var sum: Float = 0
                var j = i
                while j < i + frame { sum += abs(samples[j]); j += 8 }
                if sum < bestEnergy { bestEnergy = sum; bestCut = i + frame / 2 }
                i += frame
            }
            bounds.append(start..<bestCut)
            start = bestCut
        }
        return bounds
    }

    /// Длина самой длинной цепочки одинаковых подряд идущих сегментов.
    static func longestRepeatRun(_ segments: [Segment]) -> Int {
        var best = 0, run = 0, prev = ""
        for s in segments {
            let key = s.text.lowercased().trimmingCharacters(in: .whitespacesAndNewlines)
            run = (key == prev) ? run + 1 : 1
            prev = key
            best = max(best, run)
        }
        return best
    }

    private func transcribeChunk(samples: [Float],
                                 language: String?,
                                 temperature: Float = 0,
                                 onProgress: @escaping (Double) -> Void) throws -> [Segment] {

        try loadIfNeeded()
        guard let ctx else { throw EngineError.modelLoadFailed }

        cancelRequested = false
        progressHandler = onProgress

        var params = whisper_full_default_params(WHISPER_SAMPLING_GREEDY)
        params.n_threads        = Int32(max(2, min(8, ProcessInfo.processInfo.activeProcessorCount - 2)))
        params.print_progress   = false
        params.print_realtime   = false
        params.print_timestamps = false
        params.print_special    = false
        params.translate        = false
        params.no_timestamps    = false
        params.suppress_blank   = true
        params.suppress_nst     = true      // глушим «неречевые» токены — меньше мусора и петель
        params.no_context       = true      // не тащить прошлый текст вперёд: иначе петля кормит сама себя
        params.temperature      = temperature
        params.temperature_inc  = 0.2       // фолбэк при плохой уверенности
        params.entropy_thold    = 2.4       // сорвался в повтор → перевыбор с другой температурой
        params.logprob_thold    = -1.0
        params.no_speech_thold  = 0.6

        let selfPtr = Unmanaged.passUnretained(self).toOpaque()

        params.progress_callback_user_data = selfPtr
        params.progress_callback = { _, _, progress, userData in
            guard let userData else { return }
            let engine = Unmanaged<WhisperEngine>.fromOpaque(userData).takeUnretainedValue()
            engine.progressHandler?(Double(progress) / 100.0)
        }

        params.abort_callback_user_data = selfPtr
        params.abort_callback = { userData in
            guard let userData else { return false }
            let engine = Unmanaged<WhisperEngine>.fromOpaque(userData).takeUnretainedValue()
            return engine.cancelRequested
        }

        // Детектор речи (Silero): на вход декодеру идут только куски с речью.
        // Тишина, музыка и шум — главный источник галлюцинаций и зацикливания.
        var vadPathC: UnsafeMutablePointer<CChar>?
        if let vad = vadModelPath() {
            vadPathC = strdup(vad)
            params.vad = true
            params.vad_model_path = UnsafePointer(vadPathC)
            params.vad_params.threshold               = 0.5
            params.vad_params.min_speech_duration_ms  = 250
            params.vad_params.min_silence_duration_ms = 300
            params.vad_params.max_speech_duration_s   = 30
            params.vad_params.speech_pad_ms           = 200
            params.vad_params.samples_overlap         = 0.2
        }

        var langC: UnsafeMutablePointer<CChar>?
        if let language, language != "auto" {
            langC = strdup(language)
            params.language = UnsafePointer(langC)
        } else {
            params.language = nil
            params.detect_language = false      // whisper сам определит язык на лету
        }

        let status = whisper_full(ctx, params, samples, Int32(samples.count))
        if let langC { free(langC) }
        if let vadPathC { free(vadPathC) }

        progressHandler = nil
        if cancelRequested { throw EngineError.cancelled }
        guard status == 0 else { throw EngineError.runFailed(status) }

        var result: [Segment] = []
        let n = whisper_full_n_segments(ctx)
        for i in 0..<n {
            guard let raw = whisper_full_get_segment_text(ctx, i) else { continue }
            let text = String(cString: raw).trimmingCharacters(in: .whitespacesAndNewlines)
            if text.isEmpty { continue }
            let t0 = Double(whisper_full_get_segment_t0(ctx, i)) / 100.0
            let t1 = Double(whisper_full_get_segment_t1(ctx, i)) / 100.0
            result.append(Segment(start: t0, end: t1, text: text))
        }
        return result
    }

    /// Whisper иногда срывается в петлю и повторяет одну фразу десятки раз.
    /// VAD и фолбэк по температуре гасят почти всё, это последний рубеж:
    /// три и больше одинаковых подряд — оставляем одну.
    static func collapseRepeats(_ segments: [Segment], limit: Int = 3) -> [Segment] {
        var out: [Segment] = []
        var runText = ""
        var runCount = 0

        func flush(_ pending: [Segment]) {
            guard !pending.isEmpty else { return }
            out.append(contentsOf: runCount >= limit ? [pending[0]] : pending)
        }

        var run: [Segment] = []
        for seg in segments {
            let key = seg.text.lowercased().trimmingCharacters(in: .whitespacesAndNewlines)
            if key == runText {
                run.append(seg); runCount += 1
            } else {
                flush(run)
                run = [seg]; runText = key; runCount = 1
            }
        }
        flush(run)
        return out
    }

    /// Язык, который whisper определил сам (после прогона).
    func detectedLanguage() -> String? {
        guard let ctx else { return nil }
        let id = whisper_full_lang_id(ctx)
        guard id >= 0, let s = whisper_lang_str(id) else { return nil }
        return String(cString: s)
    }
}

// MARK: - Форматирование результата

enum Formatter2 {
    static func timecode(_ seconds: Double) -> String {
        let total = Int(seconds.rounded(.down))
        let h = total / 3600, m = (total % 3600) / 60, s = total % 60
        return h > 0 ? String(format: "%02d:%02d:%02d", h, m, s)
                     : String(format: "%02d:%02d", m, s)
    }

    static func plainText(_ segments: [Segment]) -> String {
        segments.map { $0.text }.joined(separator: " ")
                .replacingOccurrences(of: "  ", with: " ")
    }

    static func timestampedText(_ segments: [Segment]) -> String {
        segments.map { "[\(timecode($0.start)) → \(timecode($0.end))]  \($0.text)" }
                .joined(separator: "\n")
    }
}
