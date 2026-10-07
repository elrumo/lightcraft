// swift-tools-version: 6.0
// LightCraft for iPhone and iPad, built and installed with xtool (https://github.com/xtool-org/xtool)
// on Linux, Windows (WSL) or macOS: see docs/ios.md → "Build and run with xtool".
//
// The app itself is the Rust static library liblightcraft_ios.a (apps/lightcraft-ios); this package
// adds the Objective-C entry point and scene delegate (Sources/LightCraftHost) and links the system
// frameworks. xtool wraps the one library product below into the app's executable. `./run.sh`
// builds the Rust library into `.rust/` first, then runs `xtool dev`.
import Foundation
import PackageDescription

/// Where `run.sh` puts liblightcraft_ios.a (next to this file, git-ignored).
let rustLib = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent(".rust").path

/// What the Rust code calls into: winit / wgpu (UIKit, Metal, QuartzCore…), lightcraft-ios-host
/// (PhotosUI, Photos, ImageIO, UniformTypeIdentifiers), rustls' system bits (Security).
let frameworks = [
    "UIKit", "Foundation", "CoreFoundation", "CoreGraphics", "QuartzCore", "Metal", "MetalKit", "IOSurface",
    "CoreText", "Security", "AVFoundation", "CoreMedia", "CoreVideo",
    "PhotosUI", "Photos", "ImageIO", "UniformTypeIdentifiers",
]

let package = Package(
    name: "LightCraft",
    platforms: [.iOS("16.0")],
    products: [
        .library(name: "LightCraft", targets: ["LightCraftHost"]),
    ],
    targets: [
        .target(
            name: "LightCraftHost",
            linkerSettings: frameworks.map { .linkedFramework($0) } + [
                .linkedLibrary("c++"),
                .linkedLibrary("iconv"),
                .unsafeFlags(["-L", rustLib, "-llightcraft_ios"]),
            ]
        ),
    ]
)
