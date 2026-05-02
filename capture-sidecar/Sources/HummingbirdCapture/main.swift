// HummingbirdCapture — minimal ScreenCaptureKit sidecar.
//
// Protocol with the Rust core:
//   args  : --output <path.mp4>
//   stdin : line "STOP\n" → flush and exit cleanly
//   stdout: prints "READY" once capture is running, "DONE" before exiting

import AVFoundation
import CoreMedia
import Foundation
import ScreenCaptureKit

@available(macOS 13.0, *)
final class Recorder: NSObject, SCStreamOutput, SCStreamDelegate {
  private let outputURL: URL
  private var stream: SCStream?
  private var writer: AVAssetWriter?
  private var videoInput: AVAssetWriterInput?
  private var audioInput: AVAssetWriterInput?
  private var sessionStarted = false
  private let queue = DispatchQueue(label: "dev.aaronzhang.hummingbird.capture")

  init(output: URL) {
    self.outputURL = output
    super.init()
  }

  func start() async throws {
    let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
    guard let display = content.displays.first else {
      throw NSError(domain: "Hummingbird", code: 1, userInfo: [NSLocalizedDescriptionKey: "no display"])
    }
    let filter = SCContentFilter(display: display, excludingWindows: [])

    let cfg = SCStreamConfiguration()
    let scale = NSScreen.main?.backingScaleFactor ?? 2.0
    cfg.width = Int(CGFloat(display.width) * scale / 2)   // half-res for speed
    cfg.height = Int(CGFloat(display.height) * scale / 2)
    cfg.minimumFrameInterval = CMTime(value: 1, timescale: 30)
    cfg.queueDepth = 6
    cfg.showsCursor = true
    cfg.capturesAudio = true
    cfg.sampleRate = 48000
    cfg.channelCount = 2
    cfg.pixelFormat = kCVPixelFormatType_32BGRA

    if FileManager.default.fileExists(atPath: outputURL.path) {
      try? FileManager.default.removeItem(at: outputURL)
    }
    let writer = try AVAssetWriter(outputURL: outputURL, fileType: .mp4)
    let videoSettings: [String: Any] = [
      AVVideoCodecKey: AVVideoCodecType.h264,
      AVVideoWidthKey: cfg.width,
      AVVideoHeightKey: cfg.height,
      AVVideoCompressionPropertiesKey: [
        AVVideoAverageBitRateKey: 6_000_000,
        AVVideoProfileLevelKey: AVVideoProfileLevelH264HighAutoLevel,
        AVVideoMaxKeyFrameIntervalKey: 60,
        AVVideoExpectedSourceFrameRateKey: 30,
      ],
    ]
    let videoInput = AVAssetWriterInput(mediaType: .video, outputSettings: videoSettings)
    videoInput.expectsMediaDataInRealTime = true
    writer.add(videoInput)
    self.videoInput = videoInput

    let audioSettings: [String: Any] = [
      AVFormatIDKey: kAudioFormatMPEG4AAC,
      AVSampleRateKey: 48000,
      AVNumberOfChannelsKey: 2,
      AVEncoderBitRateKey: 128_000,
    ]
    let audioInput = AVAssetWriterInput(mediaType: .audio, outputSettings: audioSettings)
    audioInput.expectsMediaDataInRealTime = true
    writer.add(audioInput)
    self.audioInput = audioInput

    self.writer = writer

    let stream = SCStream(filter: filter, configuration: cfg, delegate: self)
    try stream.addStreamOutput(self, type: .screen, sampleHandlerQueue: queue)
    try stream.addStreamOutput(self, type: .audio, sampleHandlerQueue: queue)

    // Mic via AVCaptureSession routed into the asset writer's audio input.
    // For MVP simplicity we rely on SCStream's system audio capture; the
    // user's narration is picked up by the system mic loopback if their
    // mac is configured to do so. A future revision should mux a dedicated
    // AVCaptureDeviceInput for the mic into a second audio track.

    self.stream = stream
    try await stream.startCapture()
    if !writer.startWriting() {
      throw writer.error ?? NSError(domain: "Hummingbird", code: 2, userInfo: [NSLocalizedDescriptionKey: "writer start"])
    }
  }

  func stop() async {
    guard let stream = stream else { return }
    try? await stream.stopCapture()
    self.videoInput?.markAsFinished()
    self.audioInput?.markAsFinished()
    if let writer = writer {
      await writer.finishWriting()
    }
    self.stream = nil
    self.writer = nil
  }

  func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
    guard CMSampleBufferDataIsReady(sampleBuffer), let writer = writer else { return }

    if !sessionStarted {
      let pts = CMSampleBufferGetPresentationTimeStamp(sampleBuffer)
      writer.startSession(atSourceTime: pts)
      sessionStarted = true
    }

    switch type {
    case .screen:
      if let videoInput = videoInput, videoInput.isReadyForMoreMediaData {
        videoInput.append(sampleBuffer)
      }
    case .audio:
      if let audioInput = audioInput, audioInput.isReadyForMoreMediaData {
        audioInput.append(sampleBuffer)
      }
    @unknown default:
      break
    }
  }

  func stream(_ stream: SCStream, didStopWithError error: Error) {
    FileHandle.standardError.write("stream error: \(error)\n".data(using: .utf8) ?? Data())
  }
}

@available(macOS 13.0, *)
@main
struct CLI {
  static func main() async {
    var output: URL?
    var args = CommandLine.arguments.dropFirst().makeIterator()
    while let a = args.next() {
      if a == "--output", let v = args.next() {
        output = URL(fileURLWithPath: v)
      }
    }
    guard let outputURL = output else {
      fputs("missing --output\n", stderr)
      exit(64)
    }

    let recorder = Recorder(output: outputURL)
    do {
      try await recorder.start()
    } catch {
      fputs("start failed: \(error)\n", stderr)
      exit(1)
    }
    print("READY")
    fflush(stdout)

    // Wait for STOP on stdin.
    let stdin = FileHandle.standardInput
    while true {
      guard let line = readLine(strippingNewline: true) else { break }
      if line == "STOP" { break }
    }
    _ = stdin // silence unused warning

    await recorder.stop()
    print("DONE")
    fflush(stdout)
  }
}
