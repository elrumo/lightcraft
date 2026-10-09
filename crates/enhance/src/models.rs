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
    /// Who made the model, for the credit the licence may require.
    pub credit: &'static str,
    /// Where the model is described (its page), shown next to the licence.
    pub home_url: &'static str,
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
};

/// Every enhancement model, for listings.
pub const ALL: &[&ModelSpec] = &[&NOMOS_SPAN_2X];

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
    fn nomos_span_is_pinned_and_downloads_from_its_authors_repository() {
        assert_eq!(NOMOS_SPAN_2X.bytes(), 4_461_056);
        assert!(NOMOS_SPAN_2X.files.iter().all(|f| f.sha256.is_some()));
        assert!(!NOMOS_SPAN_2X.default_mirrors.is_empty());
        assert!(NOMOS_SPAN_2X.no_mirrors_message().contains(NOMOS_SPAN_2X.mirrors_env));
    }
}
