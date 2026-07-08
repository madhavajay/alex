// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "AlexandriaBar",
    platforms: [.macOS(.v14)],
    targets: [
        .target(name: "AlexandriaBarCore"),
        .executableTarget(
            name: "AlexandriaBar",
            dependencies: ["AlexandriaBarCore"],
            resources: [.copy("Resources/logos")]
        ),
        .testTarget(
            name: "AlexandriaBarCoreTests",
            dependencies: ["AlexandriaBarCore"]
        ),
    ]
)
