// swift-tools-version:5.9
import PackageDescription

let package = Package(
  name: "HummingbirdCapture",
  platforms: [.macOS(.v13)],
  products: [
    .executable(name: "HummingbirdCapture", targets: ["HummingbirdCapture"]),
  ],
  targets: [
    .executableTarget(
      name: "HummingbirdCapture",
      path: "Sources/HummingbirdCapture"
    ),
  ]
)
