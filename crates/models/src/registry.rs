//! The models LightCraft knows how to download. A model is data: its files (pinned size and
//! SHA-256), its licence, and where it comes from. Adding one is adding a [`ModelSpec`] here.

use crate::fetch::FileSpec;

/// One downloadable model.
#[derive(Debug)]
pub struct ModelSpec {
    /// Stable id: the folder under `models/` in the settings folder, and the id in commands.
    pub id: &'static str,
    /// The model's name in messages ("SAM 3").
    pub label: &'static str,
    pub files: &'static [FileSpec],
    /// Environment variable with extra mirrors (base URLs separated by commas, spaces or
    /// newlines), tried before the built-in ones.
    pub mirrors_env: &'static str,
    /// Where LightCraft itself downloads the model from, in order (base URLs: file `f` is at
    /// `<base>/<f>`). May be empty: then the user's own mirrors are needed.
    pub default_mirrors: &'static [&'static str],
    /// The weights' licence, as named in the consent dialog.
    pub licence: &'static str,
    pub licence_url: &'static str,
}

impl ModelSpec {
    /// Total size of the pinned files, in bytes.
    pub fn bytes(&self) -> u64 {
        self.files.iter().filter_map(|f| f.size).sum()
    }

    /// The mirrors to try, in order: `env`'s (the value of [`Self::mirrors_env`]), then the
    /// mirrors file's, then [`Self::default_mirrors`].
    pub fn mirrors(&self, env: Option<&str>, file: Option<&std::path::Path>) -> Vec<String> {
        crate::fetch::mirrors(env, file, self.default_mirrors)
    }

    /// What to tell the user when there is nowhere to download from.
    pub fn no_mirrors_message(&self) -> String {
        format!(
            "no download location is configured for the {} model in this build (set {}, or put the files in the model folder yourself; see docs/ai-masks.md)",
            self.label, self.mirrors_env
        )
    }
}

/// Size of the official SAM 3 `model.safetensors` (3.44 GB).
const SAM3_WEIGHTS_SIZE: u64 = 3_439_938_512;

/// The `facebook/sam3` checkpoint files `lightcraft_segment::Sam3::load` needs.
///
/// `model.safetensors` is pinned to the official checkpoint (Hugging Face LFS SHA-256). The two
/// tokenizer files are small text files without a pinned hash yet: they are only parsed by the
/// tokenizer (never executed) and capped in size.
// TODO(maintainer): pin `vocab.json` and `merges.txt` (size + SHA-256) when the files are
// uploaded to LightCraft's CDN.
const SAM3_FILES: &[FileSpec] = &[
    FileSpec { name: "vocab.json", size: None, sha256: None, max: 16 << 20 },
    FileSpec { name: "merges.txt", size: None, sha256: None, max: 16 << 20 },
    FileSpec {
        name: "model.safetensors",
        size: Some(SAM3_WEIGHTS_SIZE),
        sha256: Some("6d06f0a5f84e435071fe6603e61d0b4cc7b40e0d39d487cfd4d67d8cc11cc14a"),
        max: SAM3_WEIGHTS_SIZE,
    },
];

/// SAM 3 (Segment Anything with Concepts, Meta 2025): Object and Describe masks.
///
/// No default mirror: Hugging Face's `facebook/sam3` is gated (each user must accept the SAM
/// License there and download with their own token), so LightCraft needs its own CDN location
/// before the in-app download works without the user's list (`LIGHTCRAFT_SAM3_MIRRORS` or the
/// mirrors file, see docs/ai-masks.md).
// TODO(maintainer): add LightCraft's CDN locations (https, primary first), e.g.
// "https://<cdn-host>/models/sam3/<revision>" — the files there must match `SAM3_FILES`.
pub const SAM3: ModelSpec = ModelSpec {
    id: "sam3",
    label: "SAM 3",
    files: SAM3_FILES,
    mirrors_env: "LIGHTCRAFT_SAM3_MIRRORS",
    default_mirrors: &[],
    licence: "SAM License (Meta)",
    licence_url: "https://github.com/facebookresearch/sam3/blob/main/LICENSE",
};

/// Every model, for listings.
pub const ALL: &[&ModelSpec] = &[&SAM3];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_folder_names_and_files_are_plain_names() {
        for (i, m) in ALL.iter().enumerate() {
            assert!(!m.id.is_empty() && m.id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'), "{}", m.id);
            assert!(ALL[i + 1..].iter().all(|o| o.id != m.id), "duplicate id {}", m.id);
            assert!(!m.files.is_empty());
            for f in m.files {
                assert!(!f.name.is_empty() && !f.name.contains(['/', '\\']) && f.name != "." && f.name != "..", "{}", f.name);
                // a pinned hash is 64 lower-case hex digits and comes with a pinned size
                if let Some(h) = f.sha256 {
                    assert!(h.len() == 64 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)), "{}", f.name);
                    assert!(f.size.is_some(), "{} pins a hash but no size", f.name);
                }
            }
        }
    }

    #[test]
    fn sam3_is_the_official_checkpoint() {
        assert_eq!(SAM3.bytes(), SAM3_WEIGHTS_SIZE);
        assert!(SAM3.no_mirrors_message().contains(SAM3.mirrors_env));
    }
}
