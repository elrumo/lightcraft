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
    /// A diffusion model of several folders (`unet/`, `vae/`, …; its files are its parts', and
    /// [`ModelSpec::files`] is empty), see `lightcraft_diffuse::models`.
    pub diffusion: Option<&'static lightcraft_diffuse::models::Spec>,
    /// The licence restricts how the model may be used (an OpenRAIL licence's list of uses that
    /// aren't allowed): the consent says so.
    pub restricted: bool,
}

/// One folder of a model's files: its name under the model's folder (`""`: the folder itself),
/// the files, and where it comes from (base URLs: file `f` is at `<base>/<f>`).
pub struct Part {
    pub dir: &'static str,
    pub files: &'static [FileSpec<'static>],
    pub builtin: Vec<String>,
}

impl ModelSpec {
    /// Total size of the pinned files, in bytes.
    pub fn bytes(&self) -> u64 {
        self.parts().iter().flat_map(|p| p.files.iter()).filter_map(|f| f.size).sum()
    }

    /// The folders the model's files go in, with their built-in download locations.
    pub fn parts(&self) -> Vec<Part> {
        let mut out = Vec::new();
        if !self.files.is_empty() {
            out.push(Part { dir: "", files: self.files, builtin: self.default_mirrors.iter().map(|m| m.to_string()).collect() });
        }
        if let Some(d) = self.diffusion {
            out.extend(d.parts.iter().map(|p| Part { dir: p.dir, files: p.files, builtin: vec![p.url(d)] }));
        }
        out
    }

    /// Where part `p` downloads from: each of the user's `mirrors` (laid out as the model's
    /// repository is, so the part is in its `<dir>/` there), then, unless `builtin` is false, the
    /// part's own location.
    pub fn part_mirrors(&self, p: &Part, mirrors: &[String], builtin: bool) -> Vec<String> {
        let user = mirrors.iter().map(|m| if p.dir.is_empty() { m.clone() } else { format!("{m}/{}", p.dir) });
        let user: Vec<String> = user.collect();
        let own = if builtin { p.builtin.clone() } else { Vec::new() };
        user.into_iter().chain(own).fold(Vec::new(), |mut v, m| {
            if !v.contains(&m) {
                v.push(m);
            }
            v
        })
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
    diffusion: None,
    restricted: false,
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
    diffusion: None,
    restricted: false,
};

/// LaMa: Generative Expand's larger model (see `lightcraft_erase::lama`, which pins the file and
/// says where it comes from). 208 MB and about ten times MI-GAN's time per pass: computers, and
/// phones with the memory for it (the engine's device profile decides).
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
    diffusion: None,
    restricted: false,
};

/// A diffusion model for Generative Expand (`lightcraft_diffuse::models`, which pins every file).
const fn diffusion(d: &'static lightcraft_diffuse::models::Spec, purpose: &'static str, restricted: bool) -> ModelSpec {
    ModelSpec {
        id: d.id,
        label: d.label,
        purpose,
        files: &[],
        mirrors_env: d.mirrors_env,
        default_mirrors: &[],
        licence: d.licence,
        licence_url: d.licence_url,
        credit: d.credit,
        home_url: d.home_url,
        diffusion: Some(d),
        restricted,
    }
}

/// Stable Diffusion 1.5 Inpainting: draws new detail; computers, and phones with a GPU and the
/// memory for it.
pub const SD15: ModelSpec = diffusion(&lightcraft_diffuse::models::SD15, "Generative Expand (Crop ▸ Expand): draws new detail at 512 pixels", true);
/// SDXL Inpainting: the most detail; computers with a GPU and 16 GB or more.
pub const SDXL: ModelSpec = diffusion(&lightcraft_diffuse::models::SDXL, "Generative Expand (Crop ▸ Expand): the most detail, 1024 pixels", true);
/// Kandinsky 2.2 Inpaint: draws new detail, Apache-2.0; computers with 16 GB or more.
pub const KANDINSKY: ModelSpec = diffusion(
    &lightcraft_diffuse::models::KANDINSKY,
    "Generative Expand (Crop ▸ Expand): draws new detail at 768 pixels, conditioned on the photo",
    false,
);

/// Every enhancement model, for listings.
pub const ALL: &[&ModelSpec] = &[&NOMOS_SPAN_2X, &MIGAN, &LAMA, &SD15, &KANDINSKY, &SDXL];

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
            assert!(!m.files.is_empty() || m.diffusion.is_some());
            for f in m.parts().iter().flat_map(|p| p.files.iter()) {
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
                m.parts().iter().all(|p| !p.builtin.is_empty() && p.builtin.iter().all(|u| u.starts_with("https://") && !u.ends_with('/'))),
                "{}",
                m.id
            );
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
    fn lama_is_pinned_and_downloads_from_its_exporters_repository() {
        assert_eq!(LAMA.bytes(), 208_044_816);
        assert!(LAMA.default_mirrors.iter().all(|u| u.starts_with("https://huggingface.co/Carve/LaMa-ONNX/resolve/")));
        assert_eq!(find("lama").map(|m| m.files[0].name), Some("lama_fp32.onnx"));
    }

    #[test]
    fn diffusion_models_are_folders_with_their_own_locations() {
        let p = SD15.parts();
        assert_eq!(p.iter().map(|p| p.dir).collect::<Vec<_>>(), ["unet", "vae", "text_encoder", "tokenizer"]);
        assert_eq!(SD15.bytes(), lightcraft_diffuse::models::SD15.bytes());
        assert!(p[0].builtin[0].ends_with("/unet") && p[0].builtin[0].contains("stable-diffusion-v1-5/stable-diffusion-inpainting/resolve/"));
        // the user's mirror first, laid out as the repository is
        let m = SD15.part_mirrors(&p[1], &["https://mirror.example/sd".to_string()], true);
        assert_eq!(m[0], "https://mirror.example/sd/vae");
        assert_eq!(m.len(), 2);
        assert_eq!(SD15.part_mirrors(&p[1], &[], false), Vec::<String>::new());
        // the image encoder comes from Kandinsky's prior
        assert!(KANDINSKY.parts()[2].builtin[0].contains("kandinsky-2-2-prior"));
        const { assert!(SD15.restricted && SDXL.restricted && !KANDINSKY.restricted && !LAMA.restricted) };
        assert_eq!(LAMA.parts().len(), 1);
        assert_eq!(LAMA.part_mirrors(&LAMA.parts()[0], &["https://m.example".into()], true)[0], "https://m.example");
    }

    #[test]
    fn nomos_span_is_pinned_and_downloads_from_its_authors_repository() {
        assert_eq!(NOMOS_SPAN_2X.bytes(), 4_461_056);
        assert!(NOMOS_SPAN_2X.files.iter().all(|f| f.sha256.is_some()));
        assert!(!NOMOS_SPAN_2X.default_mirrors.is_empty());
        assert!(NOMOS_SPAN_2X.no_mirrors_message().contains(NOMOS_SPAN_2X.mirrors_env));
    }
}
