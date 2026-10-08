//! What to download for the search model, and where from. The downloader itself (mirrors,
//! resuming, size and SHA-256 checks, pure-Rust HTTPS) is `lightcraft-fetch`.
//!
//! The weights are never part of LightCraft: the user asks for them and sees the licence first.
//! Unlike SAM 3, the SigLIP 2 repository is not gated (Apache-2.0), so its Hugging Face location,
//! pinned to one commit, is a working default; the user's own mirrors (an environment variable
//! or a mirrors file) are tried first.

use std::path::Path;

pub use lightcraft_fetch::{DownloadError, FileSpec, Options, Progress, download};

/// The `google/siglip2-base-patch16-224` files [`crate::siglip::SigLip::load`] reads, pinned by
/// size and SHA-256 (Hugging Face LFS). `max` equals `size`: a longer response is refused.
pub const SIGLIP_FILES: &[FileSpec] = &[
    FileSpec {
        name: "tokenizer.json",
        size: Some(34_363_039),
        sha256: Some("cb9140fae3ac5122c972d37adf83e1248471a38147ad76f8215c8872c6fd8322"),
        max: 34_363_039,
    },
    FileSpec {
        name: "model.safetensors",
        size: Some(SIGLIP_WEIGHTS_SIZE),
        sha256: Some("612923381c76ec5a9bed335d1c48827e3f2e506ac31b044b63b2031fadee6a0b"),
        max: SIGLIP_WEIGHTS_SIZE,
    },
];

/// Size of `model.safetensors` (1.5 GB, F32).
pub const SIGLIP_WEIGHTS_SIZE: u64 = 1_500_800_904;

/// Where the model is downloaded from by default: a base URL, file `f` being at `<base>/<f>`.
/// Pinned to a commit, so what is downloaded is what the sizes and hashes above describe.
pub const DEFAULT_MIRRORS: &[&str] = &["https://huggingface.co/google/siglip2-base-patch16-224/resolve/75de2d55ec2d0b4efc50b3e9ad70dba96a7b2fa2"];

/// Environment variable with extra mirrors (base URLs separated by commas, spaces or newlines),
/// tried before the defaults.
pub const MIRRORS_ENV: &str = "LIGHTCRAFT_VISION_MIRRORS";

/// The licence the user is shown before downloading: name and where to read it.
pub const SIGLIP_LICENCE: (&str, &str) = ("Apache License 2.0 (Google, SigLIP 2)", "https://huggingface.co/google/siglip2-base-patch16-224");

/// The mirrors to try, in order: the environment variable's, then the mirrors file's (one base
/// URL per line, `#` comments), then [`DEFAULT_MIRRORS`].
pub fn mirrors(env: Option<&str>, file: Option<&Path>) -> Vec<String> {
    lightcraft_fetch::mirrors(env, file, DEFAULT_MIRRORS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_location_is_pinned_and_the_files_are_checked() {
        let m = mirrors(None, None);
        assert_eq!(m.len(), 1);
        assert!(m[0].starts_with("https://") && m[0].contains("/resolve/") && !m[0].contains("/main"));
        for f in SIGLIP_FILES {
            assert_eq!(f.size, Some(f.max));
            assert_eq!(f.sha256.map(str::len), Some(64), "{}", f.name);
        }
        // the folder a download fills is the one the loader reads
        assert!(SIGLIP_FILES.iter().any(|f| f.name == crate::siglip::WEIGHTS_FILE));
        assert!(SIGLIP_FILES.iter().any(|f| f.name == crate::siglip::TOKENIZER_FILE));
    }

    #[test]
    fn the_users_mirrors_come_first() {
        let m = mirrors(Some("https://mirror.example/siglip/"), None);
        assert_eq!(m[0], "https://mirror.example/siglip");
        assert_eq!(m.len(), 2);
    }
}
