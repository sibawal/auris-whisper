import Foundation

// Консольная проверка: те же AudioDecoder + WhisperEngine, что и в приложении.
let args = CommandLine.arguments
guard args.count >= 3 else {
    print("использование: whispertest <модель.bin> <аудиофайл> [язык]")
    exit(2)
}
let modelPath = args[1]
let audioPath = args[2]
let lang: String? = args.count > 3 ? args[3] : nil

do {
    let t0 = Date()
    let samples = try AudioDecoder.decode(url: URL(fileURLWithPath: audioPath))
    let audioSec = Double(samples.count) / Double(WHISPER_SAMPLE_RATE)
    print("декодировано: \(samples.count) отсчётов = \(String(format: "%.2f", audioSec)) c за \(String(format: "%.2f", Date().timeIntervalSince(t0))) c")

    let engine = WhisperEngine(modelPath: modelPath)
    let tLoad = Date()
    try engine.loadIfNeeded()
    print("модель загружена за \(String(format: "%.2f", Date().timeIntervalSince(tLoad))) c")

    let tRun = Date()
    let segments = try engine.transcribe(samples: samples, language: lang) { _ in }
    let elapsed = Date().timeIntervalSince(tRun)
    print("распознано за \(String(format: "%.2f", elapsed)) c  (×\(String(format: "%.1f", audioSec/elapsed)) от реального времени)")
    print("язык: \(engine.detectedLanguage() ?? "?")")
    print("--- с таймкодами ---")
    print(Formatter2.timestampedText(segments))
    print("--- сплошным текстом ---")
    print(Formatter2.plainText(segments))
} catch {
    print("ОШИБКА: \(error.localizedDescription)")
    exit(1)
}
