// PeerCapture — minimal ScreenCaptureKit + AVCaptureSession sidecar.
//
// Protocol with the Rust core:
//   args  : --output <path.mp4>
//   stdin : line "STOP\n" → flush and exit cleanly
//   stdout: prints "READY" once capture is running, "DONE" before exiting

import AVFoundation
import CoreMedia
import Foundation
import IOKit.audio
import ScreenCaptureKit

@available(macOS 13.0, *)
final class Recorder: NSObject, SCStreamOutput, SCStreamDelegate, AVCaptureAudioDataOutputSampleBufferDelegate {
  private let outputURL: URL
  private var stream: SCStream?
  private var writer: AVAssetWriter?
  private var videoInput: AVAssetWriterInput?
  private var videoAdaptor: AVAssetWriterInputPixelBufferAdaptor?
  private var audioInput: AVAssetWriterInput?
  private var sessionStarted = false
  private var isStopping = false
  private var videoBasePTS: CMTime?
  private var audioBasePTS: CMTime?
  private var lastVideoPTS: CMTime?
  private var lastAudioPTS: CMTime?
  private let queue = DispatchQueue(label: "dev.aaronzhang.peer.capture")
  private var captureSession: AVCaptureSession?

  init(output: URL) {
    self.outputURL = output
    super.init()
  }

  private func selectMicrophone() -> AVCaptureDevice? {
    // Prefer the built-in mic. Bluetooth devices like AirPods often pair as
    // A2DP output-only and deliver no packets to AVCaptureSession, producing
    // a silent track (-91 dB). The system default input may be AirPods, so
    // we explicitly skip it and only fall back to the default if no built-in
    // mic is available.
    let devices = AVCaptureDevice.devices(for: .audio)
    let defaultDevice = AVCaptureDevice.default(for: .audio)

    func summary(_ device: AVCaptureDevice) -> String {
      "\(device.localizedName) transport=\(device.transportType) connected=\(device.isConnected) suspended=\(device.isSuspended)"
    }

    for device in devices {
      FileHandle.standardError.write("mic candidate: \(summary(device))\n".data(using: .utf8) ?? Data())
    }

    if let builtIn = devices.first(where: {
      $0.isConnected && !$0.isSuspended && $0.transportType == kIOAudioDeviceTransportTypeBuiltIn
    }) {
      FileHandle.standardError.write("mic selected built-in: \(summary(builtIn))\n".data(using: .utf8) ?? Data())
      return builtIn
    }

    if let defaultDevice, defaultDevice.isConnected, !defaultDevice.isSuspended {
      FileHandle.standardError.write("mic selected default (no built-in available): \(summary(defaultDevice))\n".data(using: .utf8) ?? Data())
      return defaultDevice
    }

    if let any = devices.first(where: { $0.isConnected && !$0.isSuspended }) {
      FileHandle.standardError.write("mic selected first connected: \(summary(any))\n".data(using: .utf8) ?? Data())
      return any
    }

    if let defaultDevice {
      FileHandle.standardError.write("mic selected default as last resort: \(summary(defaultDevice))\n".data(using: .utf8) ?? Data())
      return defaultDevice
    }
    return nil
  }

  func start() async throws {
    // Mic permission. Without an embedded Info.plist with NSMicrophoneUsageDescription
    // this will return .denied without ever prompting; see Package.swift linker flags.
    if AVCaptureDevice.authorizationStatus(for: .audio) == .notDetermined {
      _ = await AVCaptureDevice.requestAccess(for: .audio)
    }
    guard AVCaptureDevice.authorizationStatus(for: .audio) == .authorized else {
      throw NSError(domain: "Peer", code: 3, userInfo: [NSLocalizedDescriptionKey: "microphone access denied"])
    }

    let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
    guard let display = content.displays.first else {
      throw NSError(domain: "Peer", code: 1, userInfo: [NSLocalizedDescriptionKey: "no display"])
    }
    let filter = SCContentFilter(display: display, excludingWindows: [])

    let cfg = SCStreamConfiguration()
    let scale = NSScreen.main?.backingScaleFactor ?? 2.0
    cfg.width = Int(CGFloat(display.width) * scale / 2)   // half-res for speed
    cfg.height = Int(CGFloat(display.height) * scale / 2)
    cfg.minimumFrameInterval = CMTime(value: 1, timescale: 30)
    cfg.queueDepth = 6
    cfg.showsCursor = true
    cfg.capturesAudio = false
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
    self.videoAdaptor = AVAssetWriterInputPixelBufferAdaptor(
      assetWriterInput: videoInput,
      sourcePixelBufferAttributes: [
        kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
        kCVPixelBufferWidthKey as String: cfg.width,
        kCVPixelBufferHeightKey as String: cfg.height,
      ]
    )

    let audioSettings: [String: Any] = [
      AVFormatIDKey: kAudioFormatMPEG4AAC,
      AVSampleRateKey: 48000,
      AVNumberOfChannelsKey: 1,
      AVEncoderBitRateKey: 96_000,
    ]
    let audioInput = AVAssetWriterInput(mediaType: .audio, outputSettings: audioSettings)
    audioInput.expectsMediaDataInRealTime = true
    writer.add(audioInput)
    self.audioInput = audioInput

    self.writer = writer

    // SCStream — video only. Mic comes from AVCaptureSession below.
    let stream = SCStream(filter: filter, configuration: cfg, delegate: self)
    try stream.addStreamOutput(self, type: .screen, sampleHandlerQueue: queue)
    self.stream = stream

    let session = AVCaptureSession()
    guard let micDevice = selectMicrophone() else {
      throw NSError(domain: "Peer", code: 4, userInfo: [NSLocalizedDescriptionKey: "no default microphone"])
    }
    let micInput = try AVCaptureDeviceInput(device: micDevice)
    if session.canAddInput(micInput) {
      session.addInput(micInput)
    } else {
      throw NSError(domain: "Peer", code: 5, userInfo: [NSLocalizedDescriptionKey: "cannot add mic input"])
    }
    let audioOutput = AVCaptureAudioDataOutput()
    audioOutput.setSampleBufferDelegate(self, queue: queue)
    if session.canAddOutput(audioOutput) {
      session.addOutput(audioOutput)
    } else {
      throw NSError(domain: "Peer", code: 6, userInfo: [NSLocalizedDescriptionKey: "cannot add audio output"])
    }
    let nc = NotificationCenter.default
    nc.addObserver(self, selector: #selector(captureSessionRuntimeError(_:)), name: .AVCaptureSessionRuntimeError, object: session)
    nc.addObserver(self, selector: #selector(captureSessionInterrupted(_:)), name: .AVCaptureSessionWasInterrupted, object: session)
    nc.addObserver(self, selector: #selector(captureSessionEnded(_:)), name: .AVCaptureSessionInterruptionEnded, object: session)
    self.captureSession = session

    // Writer must be in .writing before any sample arrives, otherwise the
    // first delegate callback hits startSession on an .unknown writer and
    // throws NSInternalInconsistencyException.
    if !writer.startWriting() {
      throw writer.error ?? NSError(domain: "Peer", code: 2, userInfo: [NSLocalizedDescriptionKey: "writer start"])
    }
    try await stream.startCapture()
    session.startRunning()
  }

  func stop() async throws {
    // Stop accepting samples before marking writer inputs finished. Both
    // ScreenCaptureKit and AVCapture can have delegate callbacks queued when
    // STOP arrives; appending one of those after finish begins fails the writer
    // and leaves the MP4 without a valid trailer.
    queue.sync {
      self.isStopping = true
    }

    self.captureSession?.stopRunning()
    if let stream = stream {
      try? await stream.stopCapture()
    }
    self.videoInput?.markAsFinished()
    self.audioInput?.markAsFinished()
    if let writer = writer {
      if writer.status == .failed {
        throw writer.error ?? NSError(domain: "Peer", code: 7, userInfo: [NSLocalizedDescriptionKey: "writer failed before finish"])
      }
      await writer.finishWriting()
      if writer.status != .completed {
        let status = writer.status.rawValue
        throw writer.error ?? NSError(domain: "Peer", code: 8, userInfo: [NSLocalizedDescriptionKey: "writer did not complete; status=\(status)"])
      }
    }
    self.stream = nil
    self.writer = nil
    self.videoAdaptor = nil
    self.captureSession = nil
  }

  // MARK: SCStreamOutput (video)

  func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
    guard !isStopping else { return }
    guard CMSampleBufferDataIsReady(sampleBuffer), let writer = writer else { return }
    if type != .screen { return }
    guard let pixelBuffer = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }

    startSessionIfNeeded()
    guard let pts = normalizedPTS(sampleBuffer, mediaType: .video) else { return }

    if let videoInput = videoInput,
       let videoAdaptor = videoAdaptor,
       videoInput.isReadyForMoreMediaData {
      if !videoAdaptor.append(pixelBuffer, withPresentationTime: pts) {
        FileHandle.standardError.write("video append failed: \(describeWriterError(writer))\n".data(using: .utf8) ?? Data())
      }
    }
  }

  // MARK: AVCaptureAudioDataOutputSampleBufferDelegate (mic)

  func captureOutput(_ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer, from connection: AVCaptureConnection) {
    guard !isStopping else { return }
    guard CMSampleBufferDataIsReady(sampleBuffer), let writer = writer else { return }

    // Anchor the writer timeline on whichever sample arrives first. Gating
    // mic on video PTS used to drop the head of the audio track and, when
    // the screen stream warmed up slowly or failed silently, every mic
    // sample — producing the "audio track is silent (-91 dB)" failure.
    startSessionIfNeeded()
    guard let normalized = normalizedSampleBuffer(sampleBuffer, mediaType: .audio) else { return }

    guard let audioInput = audioInput, audioInput.isReadyForMoreMediaData else { return }
    if !audioInput.append(normalized) {
      let fmt = CMSampleBufferGetFormatDescription(normalized)
      let asbd = fmt.flatMap { CMAudioFormatDescriptionGetStreamBasicDescription($0)?.pointee }
      let fmtStr = asbd.map { "rate=\($0.mSampleRate) ch=\($0.mChannelsPerFrame) bits=\($0.mBitsPerChannel) flags=\($0.mFormatFlags) id=\($0.mFormatID)" } ?? "nil"
      FileHandle.standardError.write("audio append failed: \(describeWriterError(writer)) sampleFmt=\(fmtStr)\n".data(using: .utf8) ?? Data())
    }
  }

  private func startSessionIfNeeded() {
    guard !sessionStarted, let writer = writer else { return }
    FileHandle.standardError.write("starting writer session at pts=0 writer.status=\(writer.status.rawValue)\n".data(using: .utf8) ?? Data())
    writer.startSession(atSourceTime: .zero)
    sessionStarted = true
  }

  private enum MediaKind {
    case video
    case audio
  }

  private func normalizedPTS(_ sampleBuffer: CMSampleBuffer, mediaType: MediaKind) -> CMTime? {
    var timing = CMSampleTimingInfo()
    let timingStatus = CMSampleBufferGetSampleTimingInfo(sampleBuffer, at: 0, timingInfoOut: &timing)
    guard timingStatus == noErr, timing.presentationTimeStamp.isValid else {
      FileHandle.standardError.write("sample timing unavailable status=\(timingStatus)\n".data(using: .utf8) ?? Data())
      return nil
    }

    let base: CMTime
    let last: CMTime?
    switch mediaType {
    case .video:
      if videoBasePTS == nil { videoBasePTS = timing.presentationTimeStamp }
      base = videoBasePTS ?? timing.presentationTimeStamp
      last = lastVideoPTS
    case .audio:
      if audioBasePTS == nil { audioBasePTS = timing.presentationTimeStamp }
      base = audioBasePTS ?? timing.presentationTimeStamp
      last = lastAudioPTS
    }

    var pts = CMTimeSubtract(timing.presentationTimeStamp, base)
    if CMTimeCompare(pts, .zero) < 0 {
      pts = .zero
    }

    if let last, CMTimeCompare(pts, last) <= 0 {
      let step = timing.duration.isValid && timing.duration.seconds > 0
        ? timing.duration
        : defaultFrameStep(for: mediaType)
      pts = CMTimeAdd(last, step)
    }

    if timing.decodeTimeStamp.isValid {
      var dts = CMTimeSubtract(timing.decodeTimeStamp, base)
      if CMTimeCompare(dts, .zero) < 0 {
        dts = .zero
      }
      timing.decodeTimeStamp = dts
    }
    timing.presentationTimeStamp = pts

    switch mediaType {
    case .video:
      lastVideoPTS = pts
    case .audio:
      lastAudioPTS = pts
    }

    return pts
  }

  private func normalizedSampleBuffer(_ sampleBuffer: CMSampleBuffer, mediaType: MediaKind) -> CMSampleBuffer? {
    var timing = CMSampleTimingInfo()
    let timingStatus = CMSampleBufferGetSampleTimingInfo(sampleBuffer, at: 0, timingInfoOut: &timing)
    guard timingStatus == noErr, timing.presentationTimeStamp.isValid else {
      FileHandle.standardError.write("sample timing unavailable status=\(timingStatus)\n".data(using: .utf8) ?? Data())
      return nil
    }

    guard let pts = normalizedPTS(sampleBuffer, mediaType: mediaType) else { return nil }
    timing.presentationTimeStamp = pts
    if timing.decodeTimeStamp.isValid {
      let base: CMTime
      switch mediaType {
      case .video:
        base = videoBasePTS ?? timing.decodeTimeStamp
      case .audio:
        base = audioBasePTS ?? timing.decodeTimeStamp
      }
      var dts = CMTimeSubtract(timing.decodeTimeStamp, base)
      if CMTimeCompare(dts, .zero) < 0 {
        dts = .zero
      }
      timing.decodeTimeStamp = dts
    }

    var normalized: CMSampleBuffer?
    let copyStatus = CMSampleBufferCreateCopyWithNewTiming(
      allocator: kCFAllocatorDefault,
      sampleBuffer: sampleBuffer,
      sampleTimingEntryCount: 1,
      sampleTimingArray: &timing,
      sampleBufferOut: &normalized
    )
    if copyStatus != noErr {
      FileHandle.standardError.write("sample retime failed status=\(copyStatus)\n".data(using: .utf8) ?? Data())
      return nil
    }
    return normalized
  }

  private func defaultFrameStep(for mediaType: MediaKind) -> CMTime {
    switch mediaType {
    case .video:
      return CMTime(value: 1, timescale: 30)
    case .audio:
      return CMTime(value: 1, timescale: 48000)
    }
  }

  private func describeWriterError(_ writer: AVAssetWriter) -> String {
    let err = writer.error as NSError?
    let code = err.map { "\($0.code)" } ?? "nil"
    let domain = err?.domain ?? "nil"
    let reason = err?.localizedFailureReason ?? "nil"
    let desc = err?.localizedDescription ?? "nil"
    let underlying = (err?.userInfo[NSUnderlyingErrorKey] as? NSError).map { "\($0.domain):\($0.code) \($0.localizedDescription)" } ?? "nil"
    return "status=\(writer.status.rawValue) domain=\(domain) code=\(code) desc=\(desc) reason=\(reason) underlying=\(underlying)"
  }

  func stream(_ stream: SCStream, didStopWithError error: Error) {
    FileHandle.standardError.write("stream error: \(error)\n".data(using: .utf8) ?? Data())
  }

  @objc fileprivate func captureSessionRuntimeError(_ note: Notification) {
    let err = note.userInfo?[AVCaptureSessionErrorKey] as? NSError
    FileHandle.standardError.write("capture session runtime error: \(err?.localizedDescription ?? "unknown")\n".data(using: .utf8) ?? Data())
  }

  @objc fileprivate func captureSessionInterrupted(_ note: Notification) {
    FileHandle.standardError.write("capture session interrupted\n".data(using: .utf8) ?? Data())
  }

  @objc fileprivate func captureSessionEnded(_ note: Notification) {
    FileHandle.standardError.write("capture session interruption ended\n".data(using: .utf8) ?? Data())
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

    while true {
      guard let line = readLine(strippingNewline: true) else { break }
      if line == "STOP" { break }
    }

    do {
      try await recorder.stop()
    } catch {
      fputs("stop failed: \(error)\n", stderr)
      exit(2)
    }
    print("DONE")
    fflush(stdout)
  }
}
