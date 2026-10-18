//! Metal random numbers (candle-core's seed buffer, third_party/candle-core/LIGHTCRAFT.md): run with `MTL_DEBUG_LAYER=1`.
#![allow(clippy::unwrap_used)]
#![cfg(target_os = "macos")]
use candle_core::{DType, Device, Tensor};
#[test]
fn randn_metal() {
    let dev = Device::new_metal(0).unwrap();
    for dt in [DType::F32, DType::F16] {
        for n in [1usize, 3, 4096, 4 * 128 * 128, 4 * 160 * 160, 4 * 256 * 256, 4 * 1000 * 1000, 4 * 4096 * 4096 / 64 + 1] {
            let t = Tensor::zeros((1, n), dt, &dev).unwrap();
            let r = t.randn_like(0., 1.).unwrap();
            let _ = r.sum_all().unwrap().to_dtype(DType::F32).unwrap().to_scalar::<f32>().unwrap();
            eprintln!("ok {dt:?} {n}");
        }
    }
}
