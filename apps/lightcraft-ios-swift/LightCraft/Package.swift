// swift-tools-version: 6.2
// LightCraft's native iPhone and iPad app (iOS 26 and later): SwiftUI over the Rust engine, through
// the C ABI of liblightcraft_bridge.a (apps/lightcraft-ios-swift, docs/ios-swift.md).
//
// Built with xtool (https://github.com/xtool-org/xtool; Linux, Windows (WSL) or macOS) or Xcode
// (../xcode/project.yml). `../run.sh` builds the Rust library into `.rust/` first.
// `swift test` (macOS) runs LightCraftCoreTests against a macOS build of the same library
// (`../run.sh --test`). LightCraftCore and its tests also build on Linux (`cargo xtask swift-test`);
// the app target is iOS only.
//
// Presentation only (CLAUDE.md, Non-negotiables): no document logic and no image or edit-geometry
// math here, no C/C++/Objective-C sources, no third-party packages.
import Foundation
import PackageDescription

/// Where `run.sh` puts liblightcraft_bridge.a for the platform being built (git-ignored).
let rustLib = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent(".rust").path

/// What the Rust library calls into: wgpu (Metal, QuartzCore, IOSurface), rustls (Security),
/// lightcraft-ios-host on iOS (UIKit, Photos, ImageIO, UniformTypeIdentifiers). On Linux (where
/// only LightCraftCore and its tests build) the C runtime's own libraries.
let darwin: [Platform] = [.iOS, .macOS]
let apple: [LinkerSetting] = [
    "Foundation", "CoreFoundation", "CoreGraphics", "QuartzCore", "Metal", "IOSurface", "Security", "ImageIO",
].map { .linkedFramework($0, .when(platforms: darwin)) }
    + ["c++", "iconv"].map { .linkedLibrary($0, .when(platforms: darwin)) }
let iosOnly: [LinkerSetting] = ["UIKit", "Photos", "PhotosUI", "UniformTypeIdentifiers"].map {
    .linkedFramework($0, .when(platforms: [.iOS]))
}
let linux: [LinkerSetting] = ["dl", "pthread", "m", "gcc_s"].map { .linkedLibrary($0, .when(platforms: [.linux])) }

let package = Package(
    name: "LightCraft",
    platforms: [.iOS("26.0"), .macOS("26.0")],
    products: [
        // xtool wraps this library product into the app's executable
        .library(name: "LightCraft", targets: ["LightCraftApp"]),
        // the Xcode project's app target compiles Sources/LightCraftApp itself, over these
        .library(name: "LightCraftCore", targets: ["LightCraftCore"]),
        .library(name: "LightCraftImaging", targets: ["LightCraftImaging"]),
    ],
    targets: [
        // the C header of the Rust library (no C is compiled)
        .systemLibrary(name: "CLightCraftBridge"),
        // the engine as Swift sees it: requests, events, pictures (Foundation only)
        .target(
            name: "LightCraftCore",
            dependencies: ["CLightCraftBridge"],
            linkerSettings: apple + iosOnly + linux + [.unsafeFlags(["-L", rustLib])]
        ),
        // pictures as CoreGraphics images
        .target(name: "LightCraftImaging", dependencies: ["LightCraftCore"]),
        // the app (SwiftUI)
        .target(
            name: "LightCraftApp",
            dependencies: ["LightCraftCore", "LightCraftImaging"],
            swiftSettings: [.defaultIsolation(MainActor.self)]
        ),
        .testTarget(
            name: "LightCraftCoreTests",
            dependencies: ["LightCraftCore"],
            resources: [.copy("Fixtures")]
        ),
    ],
    swiftLanguageModes: [.v6]
)
