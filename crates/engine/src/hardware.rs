//! What this device is: the profile Generative Expand offers its models by, and picks the default
//! from (`expand::ExpandModel::fits`, `Session::recommended_expand_model`).
//!
//! - **Class**: a computer, a phone or tablet, or a browser.
//! - **Memory**: what the models may use. On a computer, the machine's RAM (it swaps beyond that);
//!   on a phone, what the system let the app allocate when it started (`os_proc_available_memory`,
//!   given by the host: iOS ends an app that goes past it). `None` when the platform doesn't say.
//! - **GPU**: whether the candle models run on a GPU here (Metal, macOS and iOS).
//! - **Cores**: the CPU's threads, for the models that run on it.
//!
//! Detected once per run (`DeviceProfile::detect`); a host or a test can set it
//! (`Session::set_device`).

use std::sync::OnceLock;

use serde::Serialize;

/// The kind of device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DeviceClass {
    Computer,
    Phone,
    Browser,
}

/// What this device can run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceProfile {
    pub class: DeviceClass,
    /// RAM (a computer) or what the app may allocate (a phone), in bytes.
    pub memory_bytes: Option<u64>,
    /// The candle models run on a GPU (Metal).
    pub gpu: bool,
    pub cores: usize,
}

pub const GB: u64 = 1 << 30;

impl DeviceProfile {
    /// This device (detected once; cheap after the first call).
    pub fn detect() -> DeviceProfile {
        static PROFILE: OnceLock<DeviceProfile> = OnceLock::new();
        *PROFILE.get_or_init(|| {
            let class = if cfg!(target_arch = "wasm32") {
                DeviceClass::Browser
            } else if cfg!(any(target_os = "ios", target_os = "android")) {
                DeviceClass::Phone
            } else {
                DeviceClass::Computer
            };
            DeviceProfile {
                class,
                // (a phone's limit comes from its host, `Session::set_app_memory`)
                memory_bytes: if class == DeviceClass::Computer { crate::memory::total_ram().map(|b| b as u64) } else { None },
                gpu: gpu(),
                cores: std::thread::available_parallelism().map_or(1, |n| n.get()),
            }
        })
    }

    /// Whether the memory is known to be at least `bytes`, as sizes are sold: a system reports a
    /// little less than its RAM (firmware and the kernel keep some; a 16 GB Linux PC says 15.7 GB),
    /// so up to a sixteenth less counts. `unknown` when it isn't known.
    pub fn has_memory(&self, bytes: u64, unknown: bool) -> bool {
        self.memory_bytes.map_or(unknown, |m| m.saturating_add(bytes / 16) >= bytes)
    }

    /// A short description for messages ("a computer with 16 GB, GPU").
    pub fn describe(&self) -> String {
        let kind = match self.class {
            DeviceClass::Computer => "a computer",
            DeviceClass::Phone => "a phone or tablet",
            DeviceClass::Browser => "the browser",
        };
        let mem = self.memory_bytes.map(|m| format!(" with {:.0} GB", m as f64 / GB as f64)).unwrap_or_default();
        format!("{kind}{mem}{}", if self.gpu { ", GPU" } else { "" })
    }
}

/// Whether the diffusion models have a GPU here.
fn gpu() -> bool {
    #[cfg(feature = "enhance")]
    return lightcraft_diffuse::has_gpu();
    #[cfg(not(feature = "enhance"))]
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_this_machine_once() {
        let p = DeviceProfile::detect();
        assert_eq!(p, DeviceProfile::detect());
        assert!(p.cores >= 1);
        assert_eq!(p.class, DeviceClass::Computer);
        if cfg!(target_os = "linux") {
            assert!(p.memory_bytes.is_some_and(|m| m > 256 << 20));
        }
        let q = DeviceProfile { class: DeviceClass::Phone, memory_bytes: Some(6 * GB), gpu: true, cores: 6 };
        assert!(q.has_memory(4 * GB, false) && !q.has_memory(8 * GB, true));
        assert_eq!(q.describe(), "a phone or tablet with 6 GB, GPU");
        let r = DeviceProfile { memory_bytes: None, ..q };
        assert!(r.has_memory(GB, true) && !r.has_memory(GB, false));
        // a 16 GB PC as Linux reports it counts as 16 GB, 14 GB doesn't
        let pc = |kb: u64| DeviceProfile { class: DeviceClass::Computer, memory_bytes: Some(kb * 1024), gpu: false, cores: 4 };
        assert!(pc(16_479_424).has_memory(16 * GB, false));
        assert!(!pc(14 << 20).has_memory(16 * GB, true));
    }
}
