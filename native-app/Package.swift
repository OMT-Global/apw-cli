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
    targets: [
        .target(
            name: "NativeAppLib",
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
