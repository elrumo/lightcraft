//! iOS: an app in the background may not use the GPU (its command buffers are refused). While
//! paused, renders stay on the CPU and an error the device reports doesn't stop GPU rendering for
//! the rest of the process; in the foreground an error still does. Needs no GPU adapter.

use std::sync::Arc;

use lightcraft_develop::DevelopSettings;
use lightcraft_pipeline::{RenderRequest, SourceInfo};

#[test]
fn errors_while_paused_dont_stop_the_gpu_for_good() {
    assert!(!lightcraft_gpu::paused());
    lightcraft_gpu::pause();
    assert!(lightcraft_gpu::paused());
    let why = lightcraft_gpu::unavailable_reason().unwrap_or_default();
    assert!(why.contains("background"), "{why}");

    // renders stay on the CPU, and say why
    let src = Arc::new(lightcraft_scenes::demo_library()[0].render(64, 48));
    let req = RenderRequest::fit(64, 48);
    assert!(lightcraft_gpu::render(&src, &SourceInfo::default(), &DevelopSettings::default(), &req, None).is_none());
    let fell_back = lightcraft_gpu::last_fallback().unwrap_or_default();
    assert!(fell_back.contains("background"), "{fell_back}");

    // the device reports an error meanwhile (iOS refused the work of a backgrounded app)
    lightcraft_gpu::inject_device_error("command buffer refused: background execution not permitted");
    assert!(!lightcraft_gpu::enabled());
    lightcraft_gpu::resume();
    assert!(!lightcraft_gpu::paused());
    assert!(lightcraft_gpu::enabled(), "back in the foreground the GPU is tried again: {:?}", lightcraft_gpu::unavailable_reason());

    // in the foreground a device error stops it for the rest of the process, resume or not
    lightcraft_gpu::inject_device_error("device lost");
    assert!(!lightcraft_gpu::enabled());
    lightcraft_gpu::pause();
    lightcraft_gpu::resume();
    assert!(!lightcraft_gpu::enabled());
    let why = lightcraft_gpu::unavailable_reason().unwrap_or_default();
    assert!(why.contains("device lost"), "{why}");
    lightcraft_gpu::reset_failures();
}
