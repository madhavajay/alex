// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "AlexandriaBar",
    platforms: [.macOS(.v14)],
    dependencies: [
        .package(url: "https://github.com/sparkle-project/Sparkle", from: "2.9.0"),
    ],
    targets: [
        .target(name: "AlexandriaBarCore"),
        .executableTarget(
            name: "AlexandriaBar",
            dependencies: [
                "AlexandriaBarCore",
                .product(name: "Sparkle", package: "Sparkle"),
            ],
            resources: [.copy("Resources/logos")],
            linkerSettings: [
                .unsafeFlags(["-Xlinker", "-rpath", "-Xlinker", "@executable_path/../Frameworks"]),
            ]
        ),
        .testTarget(
            name: "AlexandriaBarCoreTests",
            dependencies: ["AlexandriaBarCore"]
        ),
    ]
)
