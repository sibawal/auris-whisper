import Foundation
import AVFoundation

/// Ogg (Vorbis и Opus) macOS штатно не открывает — читаем сами через libopusfile/libvorbisfile.
/// В .ogg / .oga / .opus приходят, в частности, голосовые из мессенджеров.
enum OggDecoder {

    /// Смотрим сигнатуру, а не расширение: у голосовых оно бывает какое угодно.
    static func isOggContainer(url: URL) -> Bool {
        guard let handle = try? FileHandle(forReadingFrom: url) else { return false }
        defer { try? handle.close() }
        guard let head = try? handle.read(upToCount: 4), head.count == 4 else { return false }
        return head.elementsEqual([0x4F, 0x67, 0x67, 0x53])   // "OggS"
    }

    static func decode(url: URL) throws -> [Float] {
        if let opus = try? decodeOpus(url: url) { return opus }
        return try decodeVorbis(url: url)
    }

    // MARK: - Opus

    private static func decodeOpus(url: URL) throws -> [Float] {
        var error: Int32 = 0
        guard let file = url.path.withCString({ op_open_file($0, &error) }) else {
            throw AudioError.decodeFailed("это не Opus (код \(error))")
        }
        defer { op_free(file) }

        let channels = Int(op_channel_count(file, -1))
        guard channels > 0 else { throw AudioError.decodeFailed("нет каналов") }

        let resampler = try Resampler(inputRate: 48000)     // Opus всегда 48 кГц
        var out: [Float] = []
        var buffer = [Float](repeating: 0, count: 5760 * channels)

        while true {
            let frames = buffer.withUnsafeMutableBufferPointer { ptr -> Int32 in
                op_read_float(file, ptr.baseAddress, Int32(ptr.count), nil)
            }
            if frames <= 0 { break }
            let mono = downmix(buffer, frames: Int(frames), channels: channels)
            out.append(contentsOf: resampler.push(mono))
        }
        out.append(contentsOf: resampler.drain())

        guard out.count > 1600 else { throw AudioError.empty }
        return out
    }

    // MARK: - Vorbis

    private static func decodeVorbis(url: URL) throws -> [Float] {
        var vf = OggVorbis_File()
        let status = url.path.withCString { ov_fopen($0, &vf) }
        guard status == 0 else {
            throw AudioError.decodeFailed("не Vorbis и не Opus (код \(status))")
        }
        defer { ov_clear(&vf) }

        guard let info = ov_info(&vf, -1) else { throw AudioError.decodeFailed("нет заголовка Vorbis") }
        let rate = Double(info.pointee.rate)
        let channels = Int(info.pointee.channels)
        guard rate > 0, channels > 0 else { throw AudioError.decodeFailed("битый заголовок") }

        let resampler = try Resampler(inputRate: rate)
        var out: [Float] = []
        var pcm: UnsafeMutablePointer<UnsafeMutablePointer<Float>?>?
        var bitstream: Int32 = 0

        while true {
            let frames = ov_read_float(&vf, &pcm, 4096, &bitstream)
            if frames <= 0 { break }
            guard let planes = pcm else { break }

            var mono = [Float](repeating: 0, count: Int(frames))
            for ch in 0..<channels {
                guard let plane = planes[ch] else { continue }
                for i in 0..<Int(frames) { mono[i] += plane[i] }
            }
            if channels > 1 {
                let scale = 1.0 / Float(channels)
                for i in 0..<mono.count { mono[i] *= scale }
            }
            out.append(contentsOf: resampler.push(mono))
        }
        out.append(contentsOf: resampler.drain())

        guard out.count > 1600 else { throw AudioError.empty }
        return out
    }

    // MARK: - Вспомогательное

    private static func downmix(_ interleaved: [Float], frames: Int, channels: Int) -> [Float] {
        if channels == 1 { return Array(interleaved[0..<frames]) }
        var mono = [Float](repeating: 0, count: frames)
        let scale = 1.0 / Float(channels)
        for i in 0..<frames {
            var sum: Float = 0
            for ch in 0..<channels { sum += interleaved[i * channels + ch] }
            mono[i] = sum * scale
        }
        return mono
    }
}

/// Приведение моно-потока любой частоты к 16 кГц, которые нужны whisper.
/// Один экземпляр на файл — конвертер держит состояние, поэтому на стыках блоков нет щелчков.
final class Resampler {
    private let converter: AVAudioConverter
    private let inFormat: AVAudioFormat
    private let outFormat: AVAudioFormat
    private let ratio: Double
    private let passthrough: Bool

    init(inputRate: Double) throws {
        guard let inF = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: inputRate,
                                      channels: 1, interleaved: false),
              let outF = AVAudioFormat(commonFormat: .pcmFormatFloat32,
                                       sampleRate: Double(WHISPER_SAMPLE_RATE),
                                       channels: 1, interleaved: false),
              let conv = AVAudioConverter(from: inF, to: outF) else {
            throw AudioError.decodeFailed("не удалось создать ресемплер")
        }
        inFormat = inF
        outFormat = outF
        converter = conv
        ratio = Double(WHISPER_SAMPLE_RATE) / inputRate
        passthrough = (Int(inputRate) == Int(WHISPER_SAMPLE_RATE))
    }

    func push(_ mono: [Float]) -> [Float] {
        guard !mono.isEmpty else { return [] }
        if passthrough { return mono }

        guard let inBuf = AVAudioPCMBuffer(pcmFormat: inFormat,
                                           frameCapacity: AVAudioFrameCount(mono.count)) else { return [] }
        inBuf.frameLength = AVAudioFrameCount(mono.count)
        mono.withUnsafeBufferPointer { src in
            inBuf.floatChannelData![0].update(from: src.baseAddress!, count: mono.count)
        }

        let capacity = AVAudioFrameCount(Double(mono.count) * ratio) + 1024
        guard let outBuf = AVAudioPCMBuffer(pcmFormat: outFormat, frameCapacity: capacity) else { return [] }

        var consumed = false
        var error: NSError?
        converter.convert(to: outBuf, error: &error) { _, status in
            if consumed { status.pointee = .noDataNow; return nil }
            consumed = true
            status.pointee = .haveData
            return inBuf
        }
        guard error == nil, outBuf.frameLength > 0, let ch = outBuf.floatChannelData?[0] else { return [] }
        return Array(UnsafeBufferPointer(start: ch, count: Int(outBuf.frameLength)))
    }

    /// Дотянуть хвост, который конвертер придержал внутри себя.
    func drain() -> [Float] {
        if passthrough { return [] }
        guard let outBuf = AVAudioPCMBuffer(pcmFormat: outFormat, frameCapacity: 4096) else { return [] }
        var error: NSError?
        converter.convert(to: outBuf, error: &error) { _, status in
            status.pointee = .endOfStream
            return nil
        }
        guard error == nil, outBuf.frameLength > 0, let ch = outBuf.floatChannelData?[0] else { return [] }
        return Array(UnsafeBufferPointer(start: ch, count: Int(outBuf.frameLength)))
    }
}
