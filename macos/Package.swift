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
    .target(name: "AlexCore"),
]
var testTargetDependencies: [Target.Dependency] = ["AlexCore"]

#if os(macOS)
packageTargets.append(
    .executableTarget(
        name: "Alex",
        dependencies: [
            "AlexCore",
            .product(name: "Sparkle", package: "Sparkle"),
        ],
        resources: [.copy("Resources/logos"), .copy("Resources/onboarding")],
        linkerSettings: [
            .unsafeFlags(["-Xlinker", "-rpath", "-Xlinker", "@executable_path/../Frameworks"]),
        ]
    ))
testTargetDependencies.append("Alex")
#endif

packageTargets.append(
    .testTarget(
        name: "AlexCoreTests",
        dependencies: testTargetDependencies
    ))

let package = Package(
    name: "Alex",
    platforms: supportedPlatforms,
    dependencies: packageDependencies,
    targets: packageTargets
)
