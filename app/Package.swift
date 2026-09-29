// swift-tools-version:6.0
import PackageDescription

let package = Package(
    name: "Rival",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "RivalKit", targets: ["RivalKit"]),
        .executable(name: "RivalApp", targets: ["RivalApp"]),
    ],
    targets: [
        .target(name: "RivalKit"),
        .executableTarget(name: "RivalApp", dependencies: ["RivalKit"]),
        // Fixtures live in Tests/Fixtures and are read through #filePath, so
        // the test target needs no resource bundle.
        .testTarget(name: "RivalKitTests", dependencies: ["RivalKit"]),
    ],
    swiftLanguageModes: [.v6]
)
