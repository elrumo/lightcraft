//! The enhancement models: what each is, where it comes from, and how it is pinned. The downloader
//! itself (mirrors, resuming, size and SHA-256 checks, pure-Rust HTTPS) is `lightcraft-fetch`;
//! what to download belongs to the feature, as for SAM 3 (`lightcraft-segment`) and the search
//! models (`lightcraft-vision`). The weights are never part of LightCraft: the user asks for them
//! and sees the licence first.

use lightcraft_fetch::FileSpec;

/// One downloadable enhancement model.
#[derive(Debug)]
pub struct ModelSpec {
    /// Stable id: the folder under `models/` in the settings folder, and the id in commands.
    pub id: &'static str,
    /// The model's name in messages.
    pub label: &'static str,
    /// What it does for the user.
    pub purpose: &'static str,
    pub files: &'static [FileSpec<'static>],
    /// Environment variable with extra mirrors (base URLs separated by commas, spaces or
    /// newlines), tried before the built-in ones.
    pub mirrors_env: &'static str,
    /// Where LightCraft itself downloads the model from, in order (base URLs: file `f` is at
    /// `<base>/<f>`). May be empty: then the user's own mirrors are needed.
    pub default_mirrors: &'static [&'static str],
    /// The weights' licence, as named in the consent dialog.
    pub licence: &'static str,
    pub licence_url: &'static str,
    /// Who made the model, for the credit the licence may require.
    pub credit: &'static str,
    /// Where the model is described (its page), shown next to the licence.
    pub home_url: &'static str,
    /// Offered on computers only (too large or slow for a phone or tablet).
    pub desktop_only: bool,
}

impl ModelSpec {
    /// Total size of the pinned files, in bytes.
    pub fn bytes(&self) -> u64 {
        self.files.iter().filter_map(|f| f.size).sum()
    }

    /// The mirrors to try, in order: `env`'s (the value of [`Self::mirrors_env`]), then the
    /// mirrors file's, then [`Self::default_mirrors`].
    pub fn mirrors(&self, env: Option<&str>, file: Option<&std::path::Path>) -> Vec<String> {
        lightcraft_fetch::mirrors(env, file, self.default_mirrors)
    }

    /// What to tell the user when there is nowhere to download from.
    pub fn no_mirrors_message(&self) -> String {
        format!(
            "no download location is configured for the {} model in this build (set {}, or put the files in the model folder yourself; see docs/ai-masks.md)",
            self.label, self.mirrors_env
        )
    }
}

/// Nomos Uni SPAN 2×: AI Super Resolution (see [`crate::Span`]).
///
/// Downloaded straight from its author's Hugging Face repository (public, no account needed):
/// LightCraft hosts nothing. The file is pinned, so a changed or tampered file is refused.
pub const NOMOS_SPAN_2X: ModelSpec = ModelSpec {
    id: "nomos-span-2x",
    label: "Nomos Uni SPAN 2×",
    purpose: "Super Resolution (Photo → Enhance)",
    files: &[FileSpec {
        name: "2xNomosUni_span_multijpg.safetensors",
        size: Some(4_461_056),
        sha256: Some("bee2a9c082f2b8f6e7f5db504b36593c24a1a959511f587114c399ca58b9c92c"),
        max: 4_461_056,
    }],
    mirrors_env: "LIGHTCRAFT_NOMOS_SPAN_MIRRORS",
    default_mirrors: &["https://huggingface.co/Phips/2xNomosUni_span_multijpg/resolve/main"],
    licence: "CC-BY-4.0",
    licence_url: "https://creativecommons.org/licenses/by/4.0/",
    credit: "Nomos Uni SPAN by Philip Hofmann (Phips)",
    home_url: "https://huggingface.co/Phips/2xNomosUni_span_multijpg",
    desktop_only: false,
};

/// MI-GAN: AI Remove (see `lightcraft_erase`, which pins the file and says where it comes from).
pub const MIGAN: ModelSpec = ModelSpec {
    id: lightcraft_erase::ID,
    label: lightcraft_erase::LABEL,
    purpose: "AI Remove (Remove tool ▸ Generative AI) and Generative Expand (Crop ▸ Expand)",
    files: &[FileSpec {
        name: lightcraft_erase::FILE,
        size: Some(lightcraft_erase::FILE_SIZE),
        sha256: Some(lightcraft_erase::FILE_SHA256),
        max: lightcraft_erase::FILE_SIZE,
    }],
    mirrors_env: "LIGHTCRAFT_MIGAN_MIRRORS",
    default_mirrors: &[lightcraft_erase::BASE_URL],
    licence: lightcraft_erase::LICENCE,
    licence_url: lightcraft_erase::LICENCE_URL,
    credit: lightcraft_erase::CREDIT,
    home_url: lightcraft_erase::HOME_URL,
    desktop_only: false,
};

/// LaMa: Generative Expand's larger model (see `lightcraft_erase::lama`, which pins the file and
/// says where it comes from). 208 MB and about ten times MI-GAN's time per pass: computers only.
pub const LAMA: ModelSpec = ModelSpec {
    id: lightcraft_erase::lama::ID,
    label: lightcraft_erase::lama::LABEL,
    purpose: "Generative Expand (Crop ▸ Expand), best for wide areas",
    files: &[FileSpec {
        name: lightcraft_erase::lama::FILE,
        size: Some(lightcraft_erase::lama::FILE_SIZE),
        sha256: Some(lightcraft_erase::lama::FILE_SHA256),
        max: lightcraft_erase::lama::FILE_SIZE,
    }],
    mirrors_env: "LIGHTCRAFT_LAMA_MIRRORS",
    default_mirrors: &[lightcraft_erase::lama::BASE_URL],
    licence: lightcraft_erase::lama::LICENCE,
    licence_url: lightcraft_erase::lama::LICENCE_URL,
    credit: lightcraft_erase::lama::CREDIT,
    home_url: lightcraft_erase::lama::HOME_URL,
    desktop_only: true,
};

/// Depth Anything V2 Small: Lens Blur's depth for photos without depth data of their own (see
/// `lightcraft_depth`, which pins the file and says where it comes from).
pub const DEPTH: ModelSpec = ModelSpec {
    id: lightcraft_depth::ID,
    label: lightcraft_depth::LABEL,
    purpose: "Lens Blur and Depth Range masks on photos without depth data of their own",
    files: &[FileSpec {
        name: lightcraft_depth::FILE,
        size: Some(lightcraft_depth::FILE_SIZE),
        sha256: Some(lightcraft_depth::FILE_SHA256),
        max: lightcraft_depth::FILE_SIZE,
    }],
    mirrors_env: "LIGHTCRAFT_DEPTH_MIRRORS",
    default_mirrors: &[lightcraft_depth::BASE_URL],
    licence: lightcraft_depth::LICENCE,
    licence_url: lightcraft_depth::LICENCE_URL,
    credit: lightcraft_depth::CREDIT,
    home_url: lightcraft_depth::HOME_URL,
    desktop_only: false,
};

/// Every enhancement model, for listings.
pub const ALL: &[&ModelSpec] = &[&NOMOS_SPAN_2X, &MIGAN, &LAMA, &DEPTH];

/// The enhancement model with this id.
pub fn find(id: &str) -> Option<&'static ModelSpec> {
    ALL.iter().copied().find(|m| m.id == id)
}

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
    fn built_in_mirrors_are_https_and_every_model_is_described() {
        for m in ALL {
            assert!(m.default_mirrors.iter().all(|u| u.starts_with("https://") && !u.ends_with('/')), "{}", m.id);
            assert!(
                !m.purpose.is_empty()
                    && !m.licence.is_empty()
                    && m.licence_url.starts_with("https://")
                    && m.home_url.starts_with("https://")
                    && !m.credit.is_empty(),
                "{}",
                m.id
            );
            assert_eq!(find(m.id).map(|f| f.id), Some(m.id));
        }
        assert!(find("nope").is_none());
    }

    #[test]
    fn lama_is_pinned_desktop_only_and_downloads_from_its_exporters_repository() {
        assert_eq!(LAMA.bytes(), 208_044_816);
        const { assert!(LAMA.desktop_only && !MIGAN.desktop_only) };
        assert!(LAMA.default_mirrors.iter().all(|u| u.starts_with("https://huggingface.co/Carve/LaMa-ONNX/resolve/")));
        assert_eq!(find("lama").map(|m| m.files[0].name), Some("lama_fp32.onnx"));
    }

    #[test]
    fn nomos_span_is_pinned_and_downloads_from_its_authors_repository() {
        assert_eq!(NOMOS_SPAN_2X.bytes(), 4_461_056);
        assert!(NOMOS_SPAN_2X.files.iter().all(|f| f.sha256.is_some()));
        assert!(!NOMOS_SPAN_2X.default_mirrors.is_empty());
        assert!(NOMOS_SPAN_2X.no_mirrors_message().contains(NOMOS_SPAN_2X.mirrors_env));
    }
}
