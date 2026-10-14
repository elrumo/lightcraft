//! What each diffusion model is, where it comes from, and how every file is pinned.
//!
//! The files are the model authors' own (diffusers layout) at a fixed revision of their public
//! Hugging Face repositories (no account needed): LightCraft hosts and redistributes nothing. A
//! model is a few folders (`unet/`, `vae/`, …), each downloaded from `<base>/<folder>/<file>`
//! into `<models>/<id>/<folder>/<file>`.
//!
//! Licence verdicts, read 2026-10-14 from each repository's model card:
//!
//! - Stable Diffusion 1.5 Inpainting: CreativeML OpenRAIL-M. Use, also commercial, and
//!   redistribution are allowed; the licence lists uses that are not (its Attachment A), which
//!   whoever uses the model accepts and passes on. Not an OSI open-source licence: the app says so
//!   before the download.
//! - SDXL 1.0 Inpainting 0.1: CreativeML Open RAIL++-M, the same kind of terms.
//! - Kandinsky 2.2 (decoder-inpaint and the prior's image encoder): Apache-2.0.
//!
//! The training images (LAION-5B subsets) have terms of their own, not checked here.

use lightcraft_fetch::FileSpec;

/// One folder of a model's files.
#[derive(Debug)]
pub struct Part {
    /// The folder's name under the model's folder and under its base URL.
    pub dir: &'static str,
    pub files: &'static [FileSpec<'static>],
    /// Where this folder comes from when it isn't the model's [`Spec::base_url`] (Kandinsky's
    /// image encoder lives in the prior's repository).
    pub base_url: Option<&'static str>,
}

impl Part {
    /// The built-in location of this folder: `<base>/<dir>`.
    pub fn url(&self, spec: &Spec) -> String {
        format!("{}/{}", self.base_url.unwrap_or(spec.base_url), self.dir)
    }
}

/// A downloadable diffusion model.
#[derive(Debug)]
pub struct Spec {
    /// Stable id: the folder under `models/`, and the id in commands and in a stored fill.
    pub id: &'static str,
    pub label: &'static str,
    pub base_url: &'static str,
    pub parts: &'static [Part],
    pub licence: &'static str,
    pub licence_url: &'static str,
    pub credit: &'static str,
    pub home_url: &'static str,
    /// Environment variable with extra mirrors (base URLs laid out as the repository is).
    pub mirrors_env: &'static str,
}

impl Spec {
    /// Total download size.
    pub fn bytes(&self) -> u64 {
        self.parts.iter().flat_map(|p| p.files.iter()).filter_map(|f| f.size).sum()
    }

    /// The pinned file `name` in folder `dir`.
    pub fn file(&self, dir: &str, name: &str) -> Option<&'static FileSpec<'static>> {
        self.parts.iter().find(|p| p.dir == dir).and_then(|p| p.files.iter().find(|f| f.name == name))
    }
}

const fn pin(name: &'static str, size: u64, sha256: &'static str) -> FileSpec<'static> {
    FileSpec { name, size: Some(size), sha256: Some(sha256), max: size }
}

/// The weights files, in each model's `unet/`, `vae/`, `text_encoder*/` folders.
pub const UNET_FP16: &str = "diffusion_pytorch_model.fp16.safetensors";
pub const UNET_FP32: &str = "diffusion_pytorch_model.safetensors";
pub const TEXT_FP16: &str = "model.fp16.safetensors";
pub const IMAGE_ENCODER: &str = "model.safetensors";
pub const VOCAB: &str = "vocab.json";
pub const MERGES: &str = "merges.txt";

/// CLIP's vocabulary and merges, the same files in every Stable Diffusion repository.
const TOKENIZER: &[FileSpec<'static>] = &[
    pin(VOCAB, 1_059_962, "e089ad92ba36837a0d31433e555c8f45fe601ab5c221d4f607ded32d9f7a4349"),
    pin(MERGES, 524_619, "9fd691f7c8039210e0fced15865466c65820d09b63988b0174bfe25de299051a"),
];

/// Stable Diffusion 1.5 Inpainting ([`crate::sd`]): 512 pixels, the 16-bit files (~2.1 GB).
pub const SD15: Spec = Spec {
    id: "sd15-inpaint",
    label: "Stable Diffusion 1.5 Inpainting",
    base_url: "https://huggingface.co/stable-diffusion-v1-5/stable-diffusion-inpainting/resolve/8a4288a76071f7280aedbdb3253bdb9e9d5d84bb",
    parts: &[
        Part {
            dir: "unet",
            files: &[pin(UNET_FP16, 1_719_154_104, "24b788b4a777748377cc20364eea4ae113c8c42f4468c16bc8c02fdae5492af9")],
            base_url: None,
        },
        Part {
            dir: "vae",
            files: &[pin(UNET_FP16, 167_335_342, "4fbcf0ebe55a0984f5a5e00d8c4521d52359af7229bb4d81890039d2aa16dd7c")],
            base_url: None,
        },
        Part {
            dir: "text_encoder",
            files: &[pin(TEXT_FP16, 246_144_864, "77795e2023adcf39bc29a884661950380bd093cf0750a966d473d1718dc9ef4e")],
            base_url: None,
        },
        Part { dir: "tokenizer", files: TOKENIZER, base_url: None },
    ],
    licence: "CreativeML OpenRAIL-M",
    licence_url: "https://huggingface.co/spaces/CompVis/stable-diffusion-license",
    credit: "Stable Diffusion Inpainting by Robin Rombach and Patrick Esser (Runway, CompVis LMU Munich, Stability AI)",
    home_url: "https://huggingface.co/stable-diffusion-v1-5/stable-diffusion-inpainting",
    mirrors_env: "LIGHTCRAFT_SD15_MIRRORS",
};

/// SDXL 1.0 Inpainting 0.1 ([`crate::sd`]): 1024 pixels, the 16-bit files (~6.9 GB).
pub const SDXL: Spec = Spec {
    id: "sdxl-inpaint",
    label: "SDXL Inpainting",
    base_url: "https://huggingface.co/diffusers/stable-diffusion-xl-1.0-inpainting-0.1/resolve/115134f363124c53c7d878647567d04daf26e41e",
    parts: &[
        Part {
            dir: "unet",
            files: &[pin(UNET_FP16, 5_135_178_560, "6470840731e98cc16713ddf3ac7ee458c9fdbcb881a98c6727cd4a938f227d3f")],
            base_url: None,
        },
        Part {
            dir: "vae",
            files: &[pin(UNET_FP16, 167_335_338, "4ad62825e5c8b31eefb77355ba6693785619bf7d669d9b1a6fd9f19dec6d65b3")],
            base_url: None,
        },
        Part {
            dir: "text_encoder",
            files: &[pin(TEXT_FP16, 246_144_867, "fc83cf401d930147807e7c44021c164bcc5508c9d4cc0ff35f4e354685ca9cd0")],
            base_url: None,
        },
        Part {
            dir: "text_encoder_2",
            files: &[pin(TEXT_FP16, 1_389_382_884, "a8622bd41f8d359df484fdf9de091edb9337a2fd747f0a5c9c320a62e24d1fd3")],
            base_url: None,
        },
        Part { dir: "tokenizer", files: TOKENIZER, base_url: None },
        Part { dir: "tokenizer_2", files: TOKENIZER, base_url: None },
    ],
    licence: "CreativeML Open RAIL++-M",
    licence_url: "https://huggingface.co/stabilityai/stable-diffusion-xl-base-1.0/blob/main/LICENSE.md",
    credit: "SDXL Inpainting 0.1 by the Hugging Face Diffusers team, from Stable Diffusion XL 1.0 by Stability AI",
    home_url: "https://huggingface.co/diffusers/stable-diffusion-xl-1.0-inpainting-0.1",
    mirrors_env: "LIGHTCRAFT_SDXL_MIRRORS",
};

/// Kandinsky 2.2's prior repository, where its CLIP image encoder is.
const KANDINSKY_PRIOR: &str = "https://huggingface.co/kandinsky-community/kandinsky-2-2-prior/resolve/9fc51ad5732afc5d031724219d22e6c42179c5a8";

/// Kandinsky 2.2 Inpaint ([`crate::kandinsky`]): 768 pixels; the decoder's 32-bit files and the
/// prior's image encoder (~9 GB).
pub const KANDINSKY: Spec = Spec {
    id: "kandinsky22-inpaint",
    label: "Kandinsky 2.2 Inpaint",
    base_url: "https://huggingface.co/kandinsky-community/kandinsky-2-2-decoder-inpaint/resolve/db790ad5cbcabed886f069ef2710774657621702",
    parts: &[
        Part {
            dir: "unet",
            files: &[pin(UNET_FP32, 5_012_378_704, "098b846d2378b4b44a33e0bd47f89f2886b72a10200cbf9e96d5fae3c471543f")],
            base_url: None,
        },
        Part {
            dir: "movq",
            files: &[pin(UNET_FP32, 271_380_364, "43a5860fea195a7116f2471396c5cc9535fade9b63c4857d8a192ffd924b7002")],
            base_url: None,
        },
        Part {
            dir: "image_encoder",
            files: &[pin(IMAGE_ENCODER, 3_689_912_664, "657723e09f46a7c3957df651601029f66b1748afb12b419816330f16ed45d64d")],
            base_url: Some(KANDINSKY_PRIOR),
        },
    ],
    licence: "Apache-2.0",
    licence_url: "https://www.apache.org/licenses/LICENSE-2.0",
    credit: "Kandinsky 2.2 by Arseniy Shakhmatov, Anton Razzhigaev, Aleksandr Nikolich, Vladimir Arkhipkin, Igor Pavlov, Andrey Kuznetsov and Denis Dimitrov (Sber AI, AIRI)",
    home_url: "https://huggingface.co/kandinsky-community/kandinsky-2-2-decoder-inpaint",
    mirrors_env: "LIGHTCRAFT_KANDINSKY_MIRRORS",
};

/// Every diffusion model.
pub const ALL: &[&Spec] = &[&SD15, &SDXL, &KANDINSKY];

/// The model with this id.
pub fn find(id: &str) -> Option<&'static Spec> {
    ALL.iter().copied().find(|m| m.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_file_is_pinned_and_every_model_described() {
        for m in ALL {
            assert!(m.id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'), "{}", m.id);
            assert!(m.base_url.starts_with("https://huggingface.co/") && m.base_url.contains("/resolve/") && !m.base_url.ends_with('/'));
            assert!(m.licence_url.starts_with("https://") && m.home_url.starts_with("https://") && !m.credit.is_empty());
            for p in m.parts {
                assert!(!p.dir.is_empty() && !p.dir.contains(['/', '\\', '.']), "{}", p.dir);
                assert!(p.url(m).starts_with("https://huggingface.co/"));
                for f in p.files {
                    let h = f.sha256.unwrap();
                    assert!(h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
                    assert_eq!(f.size, Some(f.max));
                }
            }
            assert_eq!(find(m.id).map(|f| f.id), Some(m.id));
        }
        assert!(find("dall-e").is_none());
        // the sizes the app names before a download
        assert_eq!(SD15.bytes(), 2_134_218_891);
        assert!((6_900_000_000..7_100_000_000).contains(&SDXL.bytes()));
        assert!((8_900_000_000..9_100_000_000).contains(&KANDINSKY.bytes()));
        assert!(KANDINSKY.parts[2].url(&KANDINSKY).contains("kandinsky-2-2-prior"));
    }
}
