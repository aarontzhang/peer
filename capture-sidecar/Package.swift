// swift-tools-version:5.9
import PackageDescription

let package = Package(
  name: "PeerCapture",
  platforms: [.macOS(.v13)],
  products: [
    .executable(name: "PeerCapture", targets: ["PeerCapture"]),
  ],
  targets: [
    .executableTarget(
      name: "PeerCapture",
      path: "Sources/PeerCapture"
    ),
  ]
)
