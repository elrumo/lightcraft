// The C interface between the Objective-C host (this target) and the Rust app
// (apps/lightcraft-ios, the static library liblightcraft_ios.a).
#pragma once

/// Runs the app (winit calls UIApplicationMain); implemented in Rust.
void lightcraft_ios_main(void);

/// One log line to the system log (NSLog: Console.app, `idevicesyslog`); called from Rust
/// (lightcraft-ios-host).
void lightcraft_host_log(const char *line);
