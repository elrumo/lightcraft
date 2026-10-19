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
    pub files: &'static [FileSpec<'static>],
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

/// The sensitive-content classifier (Falconsai's ViT-B/16, Apache-2.0), for any CPU: the int8 ONNX export the
/// onnx-community made of it, pinned to a commit. Its one file goes straight into the model's folder.
pub const NSFW_PARTS: &[OcrPart] = &[OcrPart {
    dir: "",
    files: &[FileSpec {
        name: "model_int8.onnx",
        size: Some(87_333_629),
        sha256: Some("bdc941491cd09ae75c5f40b038a5cd64482fa4e2b5297747323c5cda31303920"),
        max: 87_333_629,
    }],
    default_mirror: "https://huggingface.co/onnx-community/nsfw_image_detection-ONNX/resolve/1ceb3c7fe1e9f3f2507e6df577437f23a9149fd5/onnx",
}];

/// The same classifier for a GPU (a Mac, an iPhone): the original weights, pinned to a commit of Falconsai's own
/// repository. A download of its own, beside the CPU one in the same folder; none where there is no GPU to run it.
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub const NSFW_GPU_PARTS: &[OcrPart] = &[OcrPart {
    dir: "",
    files: &[FileSpec {
        name: "model.safetensors",
        size: Some(343_223_968),
        sha256: Some("97b2ce64ec146884b37f98ee7944ca4891aa72f6827dc0cb10684a1cbecd5830"),
        max: 343_223_968,
    }],
    default_mirror: "https://huggingface.co/Falconsai/nsfw_image_detection/resolve/96cb0d0342c7afb80cab76ecc58b265fa44da256",
}];
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
pub const NSFW_GPU_PARTS: &[OcrPart] = &[];

/// The large sensitive-content classifier (AdamCodd's ViT-B/16 at 384 px, Apache-2.0): its int8 ONNX export, pinned
/// to a commit, in the `large` folder beside the compact model's file (which has the same name).
pub const NSFW_LARGE_PARTS: &[OcrPart] = &[OcrPart {
    dir: "large",
    files: &[FileSpec {
        name: "model_int8.onnx",
        size: Some(88_500_985),
        sha256: Some("d25aa73fe1eec78459e35ff911e2af98f652ee919b48d9c54316c86d5ff435fa"),
        max: 88_500_985,
    }],
    default_mirror: "https://huggingface.co/AdamCodd/vit-base-nsfw-detector/resolve/8587de998f441aac03fdd57a85d2e4cb808c7d64/onnx",
}];

/// The large classifier for a GPU (a Mac, an iPhone): the original weights, pinned to the same commit; a download of
/// its own, beside the large CPU one in the same folder; none where there is no GPU to run it.
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub const NSFW_LARGE_GPU_PARTS: &[OcrPart] = &[OcrPart {
    dir: "large",
    files: &[FileSpec {
        name: "model.safetensors",
        size: Some(344_391_328),
        sha256: Some("266efb8bf67c1e865a577222fbbd6ddb149b9e00ba0d2b50466a034837f026a4"),
        max: 344_391_328,
    }],
    default_mirror: "https://huggingface.co/AdamCodd/vit-base-nsfw-detector/resolve/8587de998f441aac03fdd57a85d2e4cb808c7d64",
}];
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
pub const NSFW_LARGE_GPU_PARTS: &[OcrPart] = &[];

/// Download size of the large classifier.
pub const NSFW_LARGE_BYTES: u64 = 88_500_985;
/// Download size of the large classifier's GPU weights.
pub const NSFW_LARGE_GPU_BYTES: u64 = 344_391_328;

/// The licence the user is shown before downloading the large sensitive-content classifier.
pub const NSFW_LARGE_LICENCE: (&str, &str) =
    ("Apache License 2.0 (AdamCodd, vit-base-nsfw-detector)", "https://huggingface.co/AdamCodd/vit-base-nsfw-detector");

/// Download size of the CPU classifier.
pub const NSFW_BYTES: u64 = 87_333_629;
/// Download size of the GPU weights.
pub const NSFW_GPU_BYTES: u64 = 343_223_968;

/// The licence the user is shown before downloading the sensitive-content classifier.
pub const NSFW_LICENCE: (&str, &str) =
    ("Apache License 2.0 (Falcons.ai, nsfw_image_detection)", "https://huggingface.co/Falconsai/nsfw_image_detection");

/// One download: the files, the mirrors to try for them, and the folder they go into.
pub type Download = (&'static [FileSpec<'static>], Vec<String>, std::path::PathBuf);

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

/// What to download for the sensitive-content classifier into `dir` (see [`part_downloads`]; folder `nsfw`).
pub fn nsfw_downloads(dir: &Path, env: Option<&str>, file: Option<&Path>) -> Vec<Download> {
    part_downloads(NSFW_PARTS, "nsfw", dir, env, file)
}

/// What to download for the large sensitive-content classifier into `dir` (the same folder as [`nsfw_downloads`]).
pub fn nsfw_large_downloads(dir: &Path, env: Option<&str>, file: Option<&Path>) -> Vec<Download> {
    part_downloads(NSFW_LARGE_PARTS, "nsfw", dir, env, file)
}

/// What to download for the large classifier's GPU weights into `dir` (same folder; nothing where there is no GPU).
pub fn nsfw_large_gpu_downloads(dir: &Path, env: Option<&str>, file: Option<&Path>) -> Vec<Download> {
    part_downloads(NSFW_LARGE_GPU_PARTS, "nsfw", dir, env, file)
}

/// What to download for the sensitive-content classifier's GPU weights into `dir` (same folder; nothing where there is no GPU).
pub fn nsfw_gpu_downloads(dir: &Path, env: Option<&str>, file: Option<&Path>) -> Vec<Download> {
    part_downloads(NSFW_GPU_PARTS, "nsfw", dir, env, file)
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
    fn the_sensitive_content_model_fills_the_folder_the_classifier_loads() {
        let parts = nsfw_downloads(Path::new("/m/nsfw"), Some("https://mirror.example/models"), None);
        let names: Vec<String> =
            parts.iter().flat_map(|(files, _, dir)| files.iter().map(move |f| dir.join(f.name).to_string_lossy().into_owned())).collect();
        assert_eq!(names, ["/m/nsfw/model_int8.onnx"]);
        #[cfg(feature = "onnx")]
        assert_eq!(crate::nsfw::MODEL_FILE, "model_int8.onnx");
        for (files, mirrors, _) in &parts {
            assert_eq!(mirrors, &["https://mirror.example/models/nsfw".to_string(), NSFW_PARTS[0].default_mirror.to_string()]);
            assert!(mirrors[1].contains("/resolve/") && !mirrors[1].contains("/main"));
            for f in *files {
                assert_eq!((f.size, f.sha256.map(str::len)), (Some(f.max), Some(64)), "{}", f.name);
            }
        }
        assert_eq!(parts.iter().flat_map(|(f, _, _)| f.iter()).filter_map(|f| f.size).sum::<u64>(), NSFW_BYTES);
        // the GPU weights are a download of their own, into the same folder, where there is a GPU for them
        let gpu = nsfw_gpu_downloads(Path::new("/m/nsfw"), None, None);
        assert_eq!(gpu.len(), NSFW_GPU_PARTS.len());
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        {
            assert_eq!(gpu.iter().flat_map(|(f, _, _)| f.iter()).filter_map(|f| f.size).sum::<u64>(), NSFW_GPU_BYTES);
            assert!(gpu.iter().flat_map(|(f, _, _)| f.iter()).all(|f| f.size == Some(f.max) && f.sha256.map(str::len) == Some(64)));
            #[cfg(feature = "onnx")]
            assert!(gpu.iter().flat_map(|(f, _, _)| f.iter()).any(|f| f.name == crate::nsfw::GPU_WEIGHTS_FILE));
        }
    }

    #[test]
    fn the_large_sensitive_content_model_goes_in_its_own_folder_where_the_classifier_looks() {
        let parts = nsfw_large_downloads(Path::new("/m/nsfw"), Some("https://mirror.example/models"), None);
        assert_eq!(parts.len(), 1);
        let (files, mirrors, dir) = &parts[0];
        assert_eq!(dir, &Path::new("/m/nsfw").join("large"));
        #[cfg(feature = "onnx")]
        assert_eq!(files[0].name, crate::nsfw::MODEL_FILE);
        assert_eq!(mirrors[0], "https://mirror.example/models/nsfw/large");
        assert!(mirrors[1].contains("/resolve/") && !mirrors[1].contains("/main"));
        assert_eq!((files[0].size, files[0].sha256.map(str::len)), (Some(NSFW_LARGE_BYTES), Some(64)));
    }

    #[test]
    fn the_large_models_gpu_weights_are_pinned_into_the_same_folder() {
        let gpu = nsfw_large_gpu_downloads(Path::new("/m/nsfw"), None, None);
        assert_eq!(gpu.len(), NSFW_LARGE_GPU_PARTS.len());
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        {
            let (files, mirrors, dir) = &gpu[0];
            assert_eq!(dir, &Path::new("/m/nsfw").join("large"));
            assert_eq!((files[0].size, files[0].sha256.map(str::len)), (Some(NSFW_LARGE_GPU_BYTES), Some(64)));
            assert!(mirrors[0].contains("/resolve/") && !mirrors[0].contains("/main"));
            #[cfg(feature = "onnx")]
            assert_eq!(files[0].name, crate::nsfw::GPU_WEIGHTS_FILE);
        }
    }

    #[test]
    fn the_users_mirrors_come_first() {
        let m = mirrors(Some("https://mirror.example/siglip/"), None);
        assert_eq!(m[0], "https://mirror.example/siglip");
        assert_eq!(m.len(), 2);
    }
}
