import Foundation
import AVFoundation

enum AudioError: LocalizedError {
    case noAudioTrack
    case decodeFailed(String)
    case empty

    var errorDescription: String? {
        switch self {
        case .noAudioTrack:        return tr("В файле нет звуковой дорожки.", "The file has no audio track.")
        case .decodeFailed(let m): return tr("Не удалось декодировать аудио: \(m)", "Could not decode the audio: \(m)")
        case .empty:               return tr("Аудио пустое или слишком короткое.", "The audio is empty or too short.")
        }
    }
}

/// Приводит любой поддерживаемый системой файл (mp3, m4a, wav, aiff, caf,
/// mp4, mov, aac, flac …) к тому, что нужен whisper: 16 кГц, моно, Float32.
enum AudioDecoder {

    static func decode(url: URL) throws -> [Float] {
        // Ogg/Opus система не открывает — у нас для него свой декодер.
        if OggDecoder.isOggContainer(url: url) {
            return try OggDecoder.decode(url: url)
        }
        do {
            return try decodeWithAssetReader(url: url)
        } catch {
            // Видео-контейнеры читает только AVAssetReader, а часть «чистых»
            // аудиофайлов надёжнее открывается через AVAudioFile.
            return try decodeWithAudioFile(url: url)
        }
    }

    // MARK: - Основной путь: AVAssetReader (тянет и видео, и аудио)

    private static func decodeWithAssetReader(url: URL) throws -> [Float] {
        let asset = AVURLAsset(url: url)
        guard let track = asset.tracks(withMediaType: .audio).first else {
            throw AudioError.noAudioTrack
        }

        let reader = try AVAssetReader(asset: asset)
        let settings: [String: Any] = [
            AVFormatIDKey:               kAudioFormatLinearPCM,
            AVSampleRateKey:             Double(WHISPER_SAMPLE_RATE),
            AVNumberOfChannelsKey:       1,
            AVLinearPCMBitDepthKey:      32,
            AVLinearPCMIsFloatKey:       true,
            AVLinearPCMIsBigEndianKey:   false,
            AVLinearPCMIsNonInterleaved: false
        ]
        let output = AVAssetReaderTrackOutput(track: track, outputSettings: settings)
        output.alwaysCopiesSampleData = false
        guard reader.canAdd(output) else { throw AudioError.decodeFailed("формат не поддерживается") }
        reader.add(output)

        guard reader.startReading() else {
            throw AudioError.decodeFailed(reader.error?.localizedDescription ?? "неизвестная ошибка")
        }

        var samples: [Float] = []
        samples.reserveCapacity(Int(CMTimeGetSeconds(asset.duration).isFinite
                                    ? CMTimeGetSeconds(asset.duration) * 16000 : 16000))

        while let sampleBuffer = output.copyNextSampleBuffer() {
            if let block = CMSampleBufferGetDataBuffer(sampleBuffer) {
                let length = CMBlockBufferGetDataLength(block)
                if length > 0 {
                    var bytes = [UInt8](repeating: 0, count: length)
                    CMBlockBufferCopyDataBytes(block, atOffset: 0, dataLength: length, destination: &bytes)
                    bytes.withUnsafeBytes { raw in
                        samples.append(contentsOf: raw.bindMemory(to: Float.self))
                    }
                }
            }
            CMSampleBufferInvalidate(sampleBuffer)
        }

        if reader.status == .failed {
            throw AudioError.decodeFailed(reader.error?.localizedDescription ?? "чтение прервано")
        }
        guard samples.count > 1600 else { throw AudioError.empty }   // < 0.1 c
        return samples
    }

    // MARK: - Запасной путь: AVAudioFile + AVAudioConverter

    private static func decodeWithAudioFile(url: URL) throws -> [Float] {
        let file = try AVAudioFile(forReading: url)
        guard let target = AVAudioFormat(commonFormat: .pcmFormatFloat32,
                                         sampleRate: Double(WHISPER_SAMPLE_RATE),
                                         channels: 1,
                                         interleaved: false),
              let converter = AVAudioConverter(from: file.processingFormat, to: target) else {
            throw AudioError.decodeFailed("нет конвертера")
        }

        let inChunk: AVAudioFrameCount = 16384
        var samples: [Float] = []
        var finished = false

        while !finished {
            let ratio = target.sampleRate / file.processingFormat.sampleRate
            let outCapacity = AVAudioFrameCount(Double(inChunk) * ratio) + 1024
            guard let outBuf = AVAudioPCMBuffer(pcmFormat: target, frameCapacity: outCapacity) else { break }

            var convError: NSError?
            let status = converter.convert(to: outBuf, error: &convError) { _, outStatus in
                guard let inBuf = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: inChunk) else {
                    outStatus.pointee = .endOfStream
                    return nil
                }
                do { try file.read(into: inBuf, frameCount: inChunk) } catch {
                    outStatus.pointee = .endOfStream
                    return nil
                }
                if inBuf.frameLength == 0 { outStatus.pointee = .endOfStream; return nil }
                outStatus.pointee = .haveData
                return inBuf
            }

            if let e = convError { throw AudioError.decodeFailed(e.localizedDescription) }
            if outBuf.frameLength > 0, let ch = outBuf.floatChannelData?[0] {
                samples.append(contentsOf: UnsafeBufferPointer(start: ch, count: Int(outBuf.frameLength)))
            }
            if status == .endOfStream || status == .error { finished = true }
        }

        guard samples.count > 1600 else { throw AudioError.empty }
        return samples
    }

    /// Записывает 16 кГц моно Float32 в WAV (16 бит) — чтобы можно было сохранить надиктованное.
    static func writeWAV(samples: [Float], to url: URL) throws {
        let sampleRate = Int(WHISPER_SAMPLE_RATE)
        var data = Data()
        let dataBytes = samples.count * 2
        func le32(_ v: Int) -> Data { withUnsafeBytes(of: UInt32(v).littleEndian) { Data($0) } }
        func le16(_ v: Int) -> Data { withUnsafeBytes(of: UInt16(v).littleEndian) { Data($0) } }

        data.append("RIFF".data(using: .ascii)!)
        data.append(le32(36 + dataBytes))
        data.append("WAVEfmt ".data(using: .ascii)!)
        data.append(le32(16))                       // размер fmt-чанка
        data.append(le16(1))                        // PCM
        data.append(le16(1))                        // моно
        data.append(le32(sampleRate))
        data.append(le32(sampleRate * 2))           // байт/с
        data.append(le16(2))                        // выравнивание блока
        data.append(le16(16))                       // бит на отсчёт
        data.append("data".data(using: .ascii)!)
        data.append(le32(dataBytes))

        var pcm = [Int16]()
        pcm.reserveCapacity(samples.count)
        for s in samples {
            let clamped = max(-1.0, min(1.0, s))
            pcm.append(Int16(clamped * 32767.0))
        }
        pcm.withUnsafeBufferPointer { data.append(UnsafeRawBufferPointer($0).bindMemory(to: UInt8.self)) }
        try data.write(to: url)
    }
}
