import Foundation
import SwiftUI
import AppKit
import UniformTypeIdentifiers

final class AppModel: ObservableObject {

    static let shared = AppModel()

    // Настройки (переживают перезапуск)
    @Published var uiLang: UILang {
        didSet {
            currentUILang = uiLang
            UserDefaults.standard.set(uiLang.rawValue, forKey: "uiLang")
            if !isBusy { status = tr("Готов к работе", "Ready") }
        }
    }
    @Published var language: String  { didSet { UserDefaults.standard.set(language, forKey: "language") } }
    @Published var timestamps: Bool  { didSet { UserDefaults.standard.set(timestamps, forKey: "timestamps") } }
    @Published var autoSave: Bool    { didSet { UserDefaults.standard.set(autoSave, forKey: "autoSave") } }

    // Состояние
    @Published var transcript: String = ""
    @Published var status: String = ""
    @Published var progress: Double = 0
    @Published var isBusy: Bool = false
    @Published var queueInfo: String = ""
    @Published var lastSavedURL: URL?
    @Published var errorMessage: String?

    let recorder = Recorder()
    let monitor  = SystemMonitor()
    private let engine = WhisperEngine()
    private let work = DispatchQueue(label: "whisper.work", qos: .userInitiated)

    private init() {
        let d = UserDefaults.standard
        d.register(defaults: ["language": "auto", "timestamps": false, "autoSave": true, "uiLang": "ru"])
        uiLang     = UILang(rawValue: d.string(forKey: "uiLang") ?? "ru") ?? .ru
        language   = d.string(forKey: "language") ?? "auto"
        timestamps = d.bool(forKey: "timestamps")
        autoSave   = d.bool(forKey: "autoSave")
        currentUILang = uiLang
        status = tr("Готов к работе", "Ready")
    }

    /// Языки распознавания. Названия — на самих языках, переводим только «автоматически».
    var languageOptions: [(code: String, title: String)] {
        [("auto", tr("Определить автоматически", "Detect automatically")),
         ("ru", "Русский"), ("en", "English"), ("de", "Deutsch"), ("fr", "Français"),
         ("es", "Español"), ("it", "Italiano"), ("uk", "Українська"),
         ("zh", "中文"), ("ja", "日本語")]
    }

    // MARK: - Файлы

    func openPanel() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = false
        var types: [UTType] = [.audio, .movie, .mpeg4Audio, .mp3, .wav, .aiff, .mpeg4Movie, .quickTimeMovie]
        types += ["ogg", "oga", "opus", "flac", "webm", "amr"].compactMap { UTType(filenameExtension: $0) }
        panel.allowedContentTypes = types
        panel.prompt  = tr("Расшифровать", "Transcribe")
        panel.message = tr("Выберите аудио- или видеофайл", "Choose an audio or video file")
        if panel.runModal() == .OK {
            process(urls: panel.urls)
        }
    }

    func process(urls: [URL]) {
        guard !urls.isEmpty, !isBusy else { return }
        isBusy = true
        progress = 0
        errorMessage = nil

        work.async { [weak self] in
            guard let self else { return }
            for (index, url) in urls.enumerated() {
                if urls.count > 1 {
                    DispatchQueue.main.async {
                        self.queueInfo = tr("Файл \(index + 1) из \(urls.count)",
                                            "File \(index + 1) of \(urls.count)")
                    }
                }
                self.runOne(url: url, showHeader: urls.count > 1)
            }
            DispatchQueue.main.async {
                self.isBusy = false
                self.progress = 0
                self.queueInfo = ""
            }
        }
    }

    private func runOne(url: URL, showHeader: Bool) {
        let name = url.lastPathComponent
        DispatchQueue.main.async { self.status = tr("Читаю «\(name)»…", "Reading “\(name)”…") }

        do {
            let samples = try AudioDecoder.decode(url: url)
            let audioSeconds = Double(samples.count) / Double(WHISPER_SAMPLE_RATE)
            let busyText = tr("Расшифровываю «\(name)» (\(Formatter2.timecode(audioSeconds)))…",
                              "Transcribing “\(name)” (\(Formatter2.timecode(audioSeconds)))…")

            DispatchQueue.main.async {
                self.status = self.engine.isLoaded ? busyText : tr("Готовлю модель…", "Loading the model…")
            }

            let started = Date()
            let segments = try engine.transcribe(samples: samples,
                                                 language: language == "auto" ? nil : language) { p in
                DispatchQueue.main.async {
                    self.progress = p
                    if self.status != busyText { self.status = busyText }
                }
            }
            let elapsed = Date().timeIntervalSince(started)
            let text = timestamps ? Formatter2.timestampedText(segments) : Formatter2.plainText(segments)

            var saved: URL?
            if autoSave, !text.isEmpty {
                let out = url.deletingPathExtension().appendingPathExtension("txt")
                try? text.write(to: out, atomically: true, encoding: .utf8)
                saved = out
            }

            DispatchQueue.main.async {
                self.append(showHeader ? "=== \(name) ===\n" + text : text)
                self.lastSavedURL = saved ?? self.lastSavedURL
                self.status = self.doneLine(elapsed: elapsed,
                                            audioSeconds: audioSeconds,
                                            saved: saved,
                                            showLanguage: true)
            }
        } catch {
            DispatchQueue.main.async {
                self.status = tr("Готов к работе", "Ready")
                self.errorMessage = "\(name): \(error.localizedDescription)"
            }
        }
    }

    private func doneLine(elapsed: Double, audioSeconds: Double, saved: URL?, showLanguage: Bool) -> String {
        let speed = elapsed > 0 ? audioSeconds / elapsed : 0
        let speedText = String(format: "%.1f", speed)
        var line = tr("Готово за \(Formatter2.timecode(elapsed)) (×\(speedText) от реального времени)",
                      "Done in \(Formatter2.timecode(elapsed)) (×\(speedText) faster than real time)")
        if showLanguage, let lang = engine.detectedLanguage() {
            line += tr(" · язык: \(lang)", " · language: \(lang)")
        }
        if let saved {
            line += tr(" · сохранено в \(saved.lastPathComponent)", " · saved to \(saved.lastPathComponent)")
        }
        return line
    }

    // MARK: - Диктовка

    func toggleRecording() {
        if recorder.isRecording {
            let samples = recorder.stop()
            transcribeRecording(samples)
        } else {
            guard !isBusy else { return }
            Recorder.requestPermission { [weak self] granted in
                guard let self else { return }
                guard granted else {
                    self.errorMessage = tr(
                        "Нет доступа к микрофону. Системные настройки → Конфиденциальность и безопасность → Микрофон.",
                        "No microphone access. System Settings → Privacy & Security → Microphone.")
                    return
                }
                do {
                    try self.recorder.start()
                    self.status = tr("Идёт запись…", "Recording…")
                    self.errorMessage = nil
                } catch {
                    self.errorMessage = error.localizedDescription
                }
            }
        }
    }

    private func transcribeRecording(_ samples: [Float]) {
        guard samples.count > 16000 else {
            status = tr("Слишком короткая запись", "Recording is too short")
            return
        }
        isBusy = true
        progress = 0

        work.async { [weak self] in
            guard let self else { return }
            let audioSeconds = Double(samples.count) / Double(WHISPER_SAMPLE_RATE)
            let busyText = tr("Расшифровываю запись…", "Transcribing the recording…")
            DispatchQueue.main.async {
                self.status = self.engine.isLoaded ? busyText : tr("Готовлю модель…", "Loading the model…")
            }
            do {
                let started = Date()
                let segments = try self.engine.transcribe(samples: samples,
                                                          language: self.language == "auto" ? nil : self.language) { p in
                    DispatchQueue.main.async {
                        self.progress = p
                        if self.status != busyText { self.status = busyText }
                    }
                }
                let elapsed = Date().timeIntervalSince(started)
                let text = self.timestamps ? Formatter2.timestampedText(segments) : Formatter2.plainText(segments)

                var saved: URL?
                if self.autoSave, !text.isEmpty {
                    let dir = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
                        .appendingPathComponent("Whisper", isDirectory: true)
                    try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
                    let fmt = DateFormatter()
                    fmt.dateFormat = "yyyy-MM-dd HH-mm-ss"
                    let base = tr("Диктовка ", "Dictation ") + fmt.string(from: Date())
                    let txtURL = dir.appendingPathComponent(base + ".txt")
                    try? text.write(to: txtURL, atomically: true, encoding: .utf8)
                    try? AudioDecoder.writeWAV(samples: samples, to: dir.appendingPathComponent(base + ".wav"))
                    saved = txtURL
                }

                DispatchQueue.main.async {
                    self.append(text)
                    self.lastSavedURL = saved ?? self.lastSavedURL
                    self.status = self.doneLine(elapsed: elapsed, audioSeconds: audioSeconds,
                                                saved: saved, showLanguage: true)
                    self.isBusy = false
                    self.progress = 0
                }
            } catch {
                DispatchQueue.main.async {
                    self.errorMessage = error.localizedDescription
                    self.status = tr("Готов к работе", "Ready")
                    self.isBusy = false
                    self.progress = 0
                }
            }
        }
    }

    // MARK: - Текст

    private func append(_ text: String) {
        guard !text.isEmpty else { return }
        transcript = transcript.isEmpty ? text : transcript + "\n\n" + text
    }

    func cancel() {
        engine.cancel()
        status = tr("Отменяю…", "Cancelling…")
    }

    func copyToClipboard() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(transcript, forType: .string)
        status = tr("Текст скопирован в буфер обмена", "Text copied to the clipboard")
    }

    func saveAs() {
        guard !transcript.isEmpty else { return }
        let panel = NSSavePanel()
        panel.allowedContentTypes = [.plainText]
        panel.nameFieldStringValue = tr("Расшифровка.txt", "Transcript.txt")
        panel.canCreateDirectories = true
        if panel.runModal() == .OK, let url = panel.url {
            do {
                try transcript.write(to: url, atomically: true, encoding: .utf8)
                lastSavedURL = url
                status = tr("Сохранено: \(url.lastPathComponent)", "Saved: \(url.lastPathComponent)")
            } catch {
                errorMessage = error.localizedDescription
            }
        }
    }

    func revealLastSaved() {
        guard let url = lastSavedURL else { return }
        NSWorkspace.shared.activateFileViewerSelecting([url])
    }

    func clear() {
        transcript = ""
        status = tr("Готов к работе", "Ready")
        errorMessage = nil
        lastSavedURL = nil
    }
}
