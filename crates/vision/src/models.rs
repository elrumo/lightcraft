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

/// One folder of the text-reading models (PP-OCRv6 small, PaddleOCR, Apache-2.0): the detector and
/// the recogniser are two repositories on Hugging Face, each pinned to a commit.
pub struct OcrPart {
    /// Its folder inside the OCR model folder (`det`, `rec`), where the reader loads it from.
    pub dir: &'static str,
    pub files: &'static [FileSpec],
    pub default_mirror: &'static str,
}

pub const OCR_PARTS: &[OcrPart] = &[
    OcrPart {
        dir: "det",
        files: &[FileSpec {
            name: "inference.onnx",
            size: Some(9_880_512),
            sha256: Some("d73e0058b7a8086bbd57f3d10b8bcd4ff95363f67e06e2762b5e814fe9c9410e"),
            max: 9_880_512,
        }],
        default_mirror: "https://huggingface.co/PaddlePaddle/PP-OCRv6_small_det_onnx/resolve/28fe5895c24fd108c19eb3e8479f4ab385fbfc62",
    },
    OcrPart {
        dir: "rec",
        files: &[
            FileSpec {
                name: "inference.onnx",
                size: Some(21_159_378),
                sha256: Some("5435fd747c9e0efe15a96d0b378d5bd157e9492ed8fd80edf08f30d02fa24634"),
                max: 21_159_378,
            },
            // the character dictionary
            FileSpec {
                name: "inference.yml",
                size: Some(150_579),
                sha256: Some("ab078671bb49f06228eadccd34f1bb501e157f7a047095ffb943ba81512c77d1"),
                max: 150_579,
            },
        ],
        default_mirror: "https://huggingface.co/PaddlePaddle/PP-OCRv6_small_rec_onnx/resolve/b8f84f0b80c529de40b4fbb3544b84fa7233a513",
    },
];

/// Download size of the text-reading models.
pub const OCR_BYTES: u64 = 9_880_512 + 21_159_378 + 150_579;

/// The licence the user is shown before downloading the text-reading models.
pub const OCR_LICENCE: (&str, &str) =
    ("Apache License 2.0 (Baidu, PaddleOCR PP-OCRv6)", "https://huggingface.co/PaddlePaddle/PP-OCRv6_small_rec_onnx");

/// The face models (OpenCV Zoo): YuNet finds faces, SFace tells them apart. Two repositories on
/// Hugging Face, each pinned to a commit; both files go straight into the faces folder.
pub const FACE_PARTS: &[OcrPart] = &[
    OcrPart {
        dir: "",
        files: &[FileSpec {
            name: "face_detection_yunet_2023mar.onnx",
            size: Some(232_589),
            sha256: Some("8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4"),
            max: 232_589,
        }],
        default_mirror: "https://huggingface.co/opencv/face_detection_yunet/resolve/3cc26e7f1014a5ee5d74a42acee58bafc9d0a310",
    },
    OcrPart {
        dir: "",
        files: &[FileSpec {
            name: "face_recognition_sface_2021dec.onnx",
            size: Some(38_696_353),
            sha256: Some("0ba9fbfa01b5270c96627c4ef784da859931e02f04419c829e83484087c34e79"),
            max: 38_696_353,
        }],
        default_mirror: "https://huggingface.co/opencv/face_recognition_sface/resolve/3d7082438a6e4551e840c9b2bb60b71e8da4b524",
    },
];

/// Download size of the face models.
pub const FACE_BYTES: u64 = 232_589 + 38_696_353;

/// The licences the user is shown before downloading the face models: name and where to read it.
pub const FACE_LICENCES: [(&str, &str); 2] = [
    ("MIT License (Shiqi Yu, YuNet)", "https://huggingface.co/opencv/face_detection_yunet"),
    ("Apache License 2.0 (BUPT, SFace)", "https://huggingface.co/opencv/face_recognition_sface"),
];

/// One download: the files, the mirrors to try for them, and the folder they go into.
pub type Download = (&'static [FileSpec], Vec<String>, std::path::PathBuf);

/// What to download for `parts` into `dir`: per part, the files, the mirrors to try (the user's
/// first, each keeping the files under `<base>/<folder>/<part's folder>/`; then the pinned default)
/// and where they go.
pub fn part_downloads(parts: &'static [OcrPart], folder: &str, dir: &Path, env: Option<&str>, file: Option<&Path>) -> Vec<Download> {
    let user = lightcraft_fetch::mirrors(env, file, &[]);
    parts
        .iter()
        .map(|p| {
            let under = |b: &String| if p.dir.is_empty() { format!("{b}/{folder}") } else { format!("{b}/{folder}/{}", p.dir) };
            let mut mirrors: Vec<String> = user.iter().map(under).collect();
            mirrors.push(p.default_mirror.to_string());
            (p.files, mirrors, if p.dir.is_empty() { dir.to_path_buf() } else { dir.join(p.dir) })
        })
        .collect()
}

/// What to download for the text-reading models into `dir` (see [`part_downloads`]; folder `ocr`).
pub fn ocr_downloads(dir: &Path, env: Option<&str>, file: Option<&Path>) -> Vec<Download> {
    part_downloads(OCR_PARTS, "ocr", dir, env, file)
}

/// What to download for the face models into `dir` (see [`part_downloads`]; folder `faces`).
pub fn face_downloads(dir: &Path, env: Option<&str>, file: Option<&Path>) -> Vec<Download> {
    part_downloads(FACE_PARTS, "faces", dir, env, file)
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
    fn the_text_models_fill_the_folders_the_reader_loads() {
        let parts = ocr_downloads(Path::new("/m/ocr"), Some("https://mirror.example/models/"), None);
        assert_eq!(parts.len(), 2);
        let mut names: Vec<String> =
            parts.iter().flat_map(|(files, _, dir)| files.iter().map(move |f| dir.join(f.name).to_string_lossy().into_owned())).collect();
        names.sort();
        // (the reader's own file names are checked in the OCR tests, which need the `onnx` feature)
        assert_eq!(names, ["/m/ocr/det/inference.onnx", "/m/ocr/rec/inference.onnx", "/m/ocr/rec/inference.yml"]);
        for (files, mirrors, dir) in &parts {
            assert_eq!(mirrors.len(), 2, "the user's mirror, then the pinned default");
            let leaf = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            assert_eq!(mirrors[0], format!("https://mirror.example/models/ocr/{leaf}"));
            assert!(mirrors[1].contains("/resolve/") && !mirrors[1].contains("/main"));
            for f in *files {
                assert_eq!((f.size, f.sha256.map(str::len)), (Some(f.max), Some(64)), "{}", f.name);
            }
        }
        let total: u64 = parts.iter().flat_map(|(files, _, _)| files.iter()).filter_map(|f| f.size).sum();
        assert_eq!(total, OCR_BYTES);
    }

    #[test]
    fn the_face_models_fill_the_folder_the_finder_loads() {
        let parts = face_downloads(Path::new("/m/faces"), Some("https://mirror.example/models"), None);
        let mut names: Vec<String> =
            parts.iter().flat_map(|(files, _, dir)| files.iter().map(move |f| dir.join(f.name).to_string_lossy().into_owned())).collect();
        names.sort();
        assert_eq!(names, ["/m/faces/face_detection_yunet_2023mar.onnx", "/m/faces/face_recognition_sface_2021dec.onnx"]);
        for (files, mirrors, _) in &parts {
            assert_eq!(mirrors.len(), 2);
            assert_eq!(mirrors[0], "https://mirror.example/models/faces");
            assert!(mirrors[1].contains("/resolve/") && !mirrors[1].contains("/main"));
            for f in *files {
                assert_eq!((f.size, f.sha256.map(str::len)), (Some(f.max), Some(64)), "{}", f.name);
            }
        }
        let total: u64 = parts.iter().flat_map(|(files, _, _)| files.iter()).filter_map(|f| f.size).sum();
        assert_eq!(total, FACE_BYTES);
    }

    #[test]
    fn the_users_mirrors_come_first() {
        let m = mirrors(Some("https://mirror.example/siglip/"), None);
        assert_eq!(m[0], "https://mirror.example/siglip");
        assert_eq!(m.len(), 2);
    }
}
