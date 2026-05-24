// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "APW",
    defaultLocalization: "en",
    platforms: [
        .macOS(.v13),
    ],
    products: [
        .library(name: "NativeAppLib", targets: ["NativeAppLib"]),
        .executable(name: "APW", targets: ["APW"]),
    ],
    dependencies: [
        .package(url: "https://github.com/sparkle-project/Sparkle", from: "2.9.0"),
    ],
    targets: [
        .target(
            name: "NativeAppLib",
            dependencies: [
                .product(name: "Sparkle", package: "Sparkle"),
            ],
            path: "Sources/NativeAppLib",
            resources: [
                .process("Resources"),
            ]
        ),
        .executableTarget(
            name: "APW",
            dependencies: ["NativeAppLib"],
            path: "Sources/APW"
        ),
        .testTarget(
            name: "NativeAppTests",
            dependencies: ["NativeAppLib"],
            path: "Tests/NativeAppTests"
        ),
    ]
)
