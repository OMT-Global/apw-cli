// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "APWNativeHost",
    platforms: [
        .macOS(.v13),
    ],
    products: [
        .executable(name: "APWNativeHost", targets: ["APWNativeHost"]),
    ],
    targets: [
        .executableTarget(
            name: "APWNativeHost",
            path: "Sources"
        ),
    ]
)
