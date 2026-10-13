#if canImport(os)
import Foundation
import os

/// The Rust library's log lines (`lightcraft_ios_host::console`) into the system log: Console.app,
/// `idevicesyslog`, `xcrun simctl spawn booted log stream --predicate 'subsystem == "ai.storyteller.lightcraft"'`.
/// Called from the engine's threads.
@_cdecl("lightcraft_host_log")
public nonisolated func lightcraftHostLog(_ line: UnsafePointer<CChar>?) {
    guard let line else { return }
    let text = String(cString: line)
    Logger(subsystem: "ai.storyteller.lightcraft", category: "engine").log("\(text, privacy: .public)")
}
#endif
