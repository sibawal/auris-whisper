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
        (Bundle.main.urls(forResourcesWithExtension: "bin", subdirectory: nil) ?? []).first?.path
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

    func transcribe(samples: [Float],
                    language: String?,          // nil / "auto" → автоопределение
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
        params.temperature_inc  = 0.2       // фолбэк при плохой уверенности
        params.entropy_thold    = 2.4
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

        var status: Int32 = 0
        if let language, language != "auto" {
            language.withCString { lang in
                params.language = lang
                status = whisper_full(ctx, params, samples, Int32(samples.count))
            }
        } else {
            params.language = nil
            params.detect_language = false      // whisper сам определит язык на лету
            status = whisper_full(ctx, params, samples, Int32(samples.count))
        }

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
