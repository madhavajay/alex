// swift-tools-version: 6.0
import PackageDescription

#if os(macOS)
let supportedPlatforms: [SupportedPlatform]? = [.macOS(.v14)]
let packageDependencies: [Package.Dependency] = [
    .package(url: "https://github.com/sparkle-project/Sparkle", from: "2.9.0"),
]
#else
let supportedPlatforms: [SupportedPlatform]? = nil
let packageDependencies: [Package.Dependency] = []
#endif

var packageTargets: [Target] = [
    .target(name: "AlexandriaBarCore"),
]

#if os(macOS)
packageTargets.append(
    .executableTarget(
        name: "AlexandriaBar",
        dependencies: [
            "AlexandriaBarCore",
            .product(name: "Sparkle", package: "Sparkle"),
        ],
        resources: [.copy("Resources/logos"), .copy("Resources/onboarding")],
        linkerSettings: [
            .unsafeFlags(["-Xlinker", "-rpath", "-Xlinker", "@executable_path/../Frameworks"]),
        ]
    ))
#endif

packageTargets.append(
    .testTarget(
        name: "AlexandriaBarCoreTests",
        dependencies: ["AlexandriaBarCore"],
        resources: [.copy("Fixtures")]
    ))

let package = Package(
    name: "AlexandriaBar",
    platforms: supportedPlatforms,
    dependencies: packageDependencies,
    targets: packageTargets
)
