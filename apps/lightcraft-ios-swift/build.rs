//! The commit System Info and About show (`LIGHTCRAFT_COMMIT`), stamped whenever the app's engine
//! library is built (`run.sh`, Xcode's build phase, CI).

include!("../../xtask/src/build_info.rs");

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../xtask/src/build_info.rs");
    emit_for_build_script(env!("CARGO_PKG_VERSION"));
}
