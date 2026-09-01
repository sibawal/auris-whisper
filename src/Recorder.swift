import Foundation
import AVFoundation

/// Запись с микрофона сразу в 16 кГц моно Float32 — ровно то, что ест whisper.
final class Recorder: ObservableObject {

    @Published var isRecording = false
    @Published var level: Float = 0          // 0…1 для индикатора
    @Published var duration: Double = 0

    private let engine = AVAudioEngine()
    private var converter: AVAudioConverter?
    private var buffer: [Float] = []
    private let bufferLock = NSLock()
    private var timer: Timer?

    var samples: [Float] {
        bufferLock.lock(); defer { bufferLock.unlock() }
        return buffer
    }

    static func requestPermission(_ done: @escaping (Bool) -> Void) {
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized:
            done(true)
        case .notDetermined:
            AVCaptureDevice.requestAccess(for: .audio) { ok in
                DispatchQueue.main.async { done(ok) }
            }
        default:
            done(false)
        }
    }

    func start() throws {
        bufferLock.lock(); buffer.removeAll(); bufferLock.unlock()

        let input = engine.inputNode
        let inputFormat = input.outputFormat(forBus: 0)
        guard inputFormat.sampleRate > 0 else {
            throw NSError(domain: "Recorder", code: 1,
                          userInfo: [NSLocalizedDescriptionKey: tr("Микрофон недоступен.", "The microphone is unavailable.")])
        }
        guard let outFormat = AVAudioFormat(commonFormat: .pcmFormatFloat32,
                                            sampleRate: Double(WHISPER_SAMPLE_RATE),
                                            channels: 1,
                                            interleaved: false),
              let conv = AVAudioConverter(from: inputFormat, to: outFormat) else {
            throw NSError(domain: "Recorder", code: 2,
                          userInfo: [NSLocalizedDescriptionKey: tr("Не удалось настроить конвертер звука.", "Could not set up the audio converter.")])
        }
        converter = conv

        input.installTap(onBus: 0, bufferSize: 4096, format: inputFormat) { [weak self] inBuf, _ in
            guard let self, let conv = self.converter else { return }
            let ratio = outFormat.sampleRate / inputFormat.sampleRate
            let capacity = AVAudioFrameCount(Double(inBuf.frameLength) * ratio) + 1024
            guard let outBuf = AVAudioPCMBuffer(pcmFormat: outFormat, frameCapacity: capacity) else { return }

            var consumed = false
            var err: NSError?
            conv.convert(to: outBuf, error: &err) { _, status in
                if consumed { status.pointee = .noDataNow; return nil }
                consumed = true
                status.pointee = .haveData
                return inBuf
            }
            guard err == nil, outBuf.frameLength > 0, let ch = outBuf.floatChannelData?[0] else { return }

            let chunk = Array(UnsafeBufferPointer(start: ch, count: Int(outBuf.frameLength)))
            var sum: Float = 0
            for v in chunk { sum += v * v }
            let rms = (chunk.isEmpty ? 0 : (sum / Float(chunk.count)).squareRoot())

            self.bufferLock.lock()
            self.buffer.append(contentsOf: chunk)
            let total = self.buffer.count
            self.bufferLock.unlock()

            DispatchQueue.main.async {
                self.level = min(1, rms * 12)
                self.duration = Double(total) / Double(WHISPER_SAMPLE_RATE)
            }
        }

        engine.prepare()
        try engine.start()
        isRecording = true
    }

    @discardableResult
    func stop() -> [Float] {
        guard isRecording else { return samples }
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
        isRecording = false
        level = 0
        timer?.invalidate(); timer = nil
        return samples
    }
}
