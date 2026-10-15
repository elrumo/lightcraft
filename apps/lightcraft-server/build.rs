//! The version and commit the admin page shows (`LIGHTCRAFT_BUILD_VERSION`, `LIGHTCRAFT_COMMIT`).

include!("../../xtask/src/build_info.rs");

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../xtask/src/build_info.rs");
    emit_for_build_script(env!("CARGO_PKG_VERSION"));
}
