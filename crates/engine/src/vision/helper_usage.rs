//! How much of a device the work for its sync server may take, and what the front end knows of the device. Plain
//! values, so every build has them (the browser builds none of the helper itself, `helper.rs`, but its library file
//! and commands still carry the choice).

/// The person has not used the device for this long: `Auto` takes a middle share of it.
const IDLE_MEDIUM: u64 = 30;
/// Nor for this long: `Auto` takes all it can.
const IDLE_HIGH: u64 = 120;

/// How much of this device the work for its server may take (`vision.setHelperUsage`, kept with the library). Below
/// `High` the helper's threads run at background priority, so the system gives everything else (the person's other
/// apps, LightCraft's own editing) the processor first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HelperUsage {
    /// Follows the device ([`HelperConditions`]): a little while the person uses it, more as it is left alone, all it
    /// can once it has been idle a while, and little again when it is hot or saving power.
    #[default]
    Auto,
    Low,
    Medium,
    High,
}

impl HelperUsage {
    pub fn key(self) -> &'static str {
        match self {
            HelperUsage::Auto => "auto",
            HelperUsage::Low => "low",
            HelperUsage::Medium => "medium",
            HelperUsage::High => "high",
        }
    }

    pub fn from_key(k: &str) -> Option<HelperUsage> {
        [HelperUsage::Auto, HelperUsage::Low, HelperUsage::Medium, HelperUsage::High].into_iter().find(|u| u.key() == k)
    }

    /// The level this choice stands for now (`Auto` decides from `c`; a front end that says nothing gets `Medium`).
    pub fn effective(self, c: &HelperConditions) -> HelperUsage {
        if self != HelperUsage::Auto {
            return self;
        }
        if c.low_power || c.thermal >= 2 {
            return HelperUsage::Low;
        }
        match c.idle_secs {
            Some(s) if s >= IDLE_HIGH && c.thermal == 0 => HelperUsage::High,
            Some(s) if s < IDLE_MEDIUM => HelperUsage::Low,
            _ => HelperUsage::Medium,
        }
    }

    /// Photos worked on at once for each part at this level, on a machine with `cores`. Fetching, decoding and building
    /// previews wait on the network and the disk and use one core each, and a small model can't keep a machine's cores
    /// busy by itself.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(super) fn workers(self, cores: usize) -> usize {
        match self {
            HelperUsage::Low => (cores / 4).clamp(1, 2),
            HelperUsage::Auto | HelperUsage::Medium => (cores / 2).clamp(2, 6),
            HelperUsage::High => (cores * 3 / 4).clamp(4, 12),
        }
    }
}

/// What the front end knows of the device, for [`HelperUsage::Auto`] (`vision.setHelperConditions`; not kept).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HelperConditions {
    /// Seconds since the person last used the device (a Mac: any app; a phone: this one), `None` when not known.
    pub idle_secs: Option<u64>,
    /// The system's thermal state: 0 nominal, 1 fair, 2 serious, 3 critical.
    pub thermal: u8,
    /// Low Power Mode is on.
    pub low_power: bool,
}
