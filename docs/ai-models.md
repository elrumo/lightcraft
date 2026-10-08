# Optional AI models

LightCraft never bundles model weights. Each model is **optional**: it is downloaded only when the user asks, after
the app names its licence, and every file is verified (exact size and SHA-256 where pinned) before use. Without a
model, the feature that needs it says so and everything else works. Results are baked (SAM 3 stores its mask
logits; denoise, super resolution and depth will store an enhanced image or a depth map), so renders and exports never
run a model.

`crates/models` holds what each model is (`registry.rs`: files with pinned size + SHA-256, licence, mirrors) and how
it is downloaded (`fetch`: ordered mirrors, resume, `.part` files, size cap, cancel; `download`: the background
thread the engine starts). Adding a model is adding a `ModelSpec` there. Inference lives in the crate that runs the
model (`crates/segment` for SAM 3).

Models whose licence allows redistribution (Apache-2.0, MIT, BSD, CC-BY) can be mirrored by LightCraft with their
licence text and attribution next to them; gated or custom-licence models (SAM 3) cannot, and need the user's own
mirror or a manual install ([ai-masks.md](ai-masks.md)).

Today: **SAM 3** (Object and Describe masks). The rest of this page is the research behind what to add next
(2026-10-08; read from each repo's LICENSE and each Hugging Face model card — **re-check the licence of a model when
you add it**, and write the verdict and the URL you read in its `ModelSpec`).

## Runtime

Pure Rust only. The route is **candle 0.9.2 + `candle-transformers` 0.9.2** (already pinned for SAM 3; Metal on
macOS, CPU elsewhere). `candle-transformers` 0.9.2 adds only pure-Rust crates (`fancy-regex`, `regex-automata`,
`aho-corasick`, `serde_plain`; checked with `cargo tree`, and it type-checks for `aarch64-apple-ios`) and ships
`siglip`, `clip`, `dinov2`, `depth_anything_v2`, `hiera`, `segment_anything` (MobileSAM's TinyViT), `mobilenetv4`.
Do not upgrade candle past 0.9.2 until `tokenizers`/Oniguruma (a C library) is out of its dependency tree.
Gaps: no FFT (rules out LaMa), no `grid_sample`, no instance norm (use GroupNorm), slow depthwise convolutions, CPU
only on Windows and Linux.

Rejected: `candle-onnx` (CPU only, needs `protoc`, lacks ConvTranspose, GridSample, LayerNorm, InstanceNorm, Einsum,
DFT), `wonnx` (archived), `tract` (x86 convolutions 3.5–5.7× slower than ONNX Runtime in its own benchmarks, and its
`build.rs` compiles assembly with `cc`). To try if Windows/Linux CPU speed is not enough: Burn + `burn-onnx` (wgpu
on all three OSes, widest op coverage; a second GPU stack, Rust generated at build time, conv speed unmeasured).

## Candidates

"Weights licence" is what the model card says; *not stated* means only the code licence is on record, which must be
settled before LightCraft hosts a mirror.

| Feature | Model | Code / weights licence | Size | Notes |
|---|---|---|---|---|
| Super resolution | Nomos Uni SPAN 2x/4x (compact 2x on CPU) | Apache-2.0 / CC-BY-4.0 | 4.5 MB (1.2 MB) | plain convs; CC-BY needs attribution; training-image provenance undocumented |
| Denoise (sRGB) | SCUNet `real_psnr` | Apache-2.0 / *not stated* | 72 MB | synthetic training data; window attention, tile; works on sRGB, not raw |
| Depth | Depth Anything V2 **Small** (DA3-BASE as upgrade) | Apache-2.0 / Apache-2.0 | 99 MB (541 MB) | in `candle-transformers`; Base/Large of V2 are CC-BY-NC |
| Face detection | YuNet | MIT / MIT | 0.2 MB | 5 landmarks |
| Face grouping | SFace | Apache-2.0 / Apache-2.0 | 37 MB | trained on MS-Celeb-1M, VGGFace2, CASIA; opt-in |
| Eyes closed, smile | MediaPipe Face Mesh V2 + eye aspect ratio | Apache-2.0 / Apache-2.0 | few MB | ships as TFLite; convert once |
| Search, similar, duplicates, keywords | SigLIP 2 B/16 (fallback SigLIP v1, OpenCLIP B/32) | Apache-2.0 / Apache-2.0 | vision 95–190 MB, text 283–565 MB | Gemma SentencePiece tokenizer needed; SigLIP 2 in candle 0.9.2 unverified |
| Personal aesthetic score | linear head on the SigLIP embedding, trained on the user's ratings | n/a | 0 | |
| Objects without SAM 3's 3.4 GB | SAM 2.1 tiny/small | Apache-2.0 / Apache-2.0 | 39–46 M params | `hiera` in `candle-transformers` |
| Sky | Sky U²-Net small | MIT / MIT | ~2 MB | training data not stated |
| Remove (large areas) | MI-GAN | MIT / MIT | 29.5 MB | separable convs, no FFT; Places2/FFHQ terms unverified |

Subject, Sky, People and Landscape masks can use SAM 3's Describe path with fixed prompts for users who have it.

## Do not use

- **Non-commercial or research-only weights:** SCRFD, ArcFace and the rest of InsightFace's models, EdgeFace,
  Depth Anything V2 Base/Large and DA3 Large/Giant, Depth Pro, RMBG-1.4/2.0, SegFormer (and fine-tunes of it),
  Sapiens, Jina-CLIP v2, MobileCLIP, MAT, Zero-DCE, Deep White-Balance, Flare7K, pyiqa (PolyForm Noncommercial).
- **Copyleft code:** RawNIND (GPL-3), YOLO-World and the YOLO face detectors, Aesthetic Predictor V2.5 (AGPL-3),
  NNDemosaicAndDenoise.
- **Trained on MIT-Adobe FiveK (Adobe-derived):** AdaInt, CSRNet, SCI and some Retinexformer weights. There is no
  neural auto tone or white balance here; the classical ones stay.
- **Custom or gated licences:** DINOv3, EdgeSAM (NTU S-Lab), PaliGemma.
- **ADE20K-trained segmenters** (UperNet, OneFormer…): the dataset is research-only; use SAM 3 prompts instead.

## Open questions

- **Raw-domain denoise has no clean open model.** Lightroom's Denoise runs on demosaiced raw; the only raw-trained
  candidate (RawNIND) is GPL-3 code with "GPL-3 + CC-BY-4.0" weights and a CC-BY-SA dataset. Ask the author, clean-room
  a U-Net from the paper, or train our own.
- Weights with no stated licence: SCUNet, MobileSAM, U²-Net, SCHP, BiSeNet; and YuNet's WIDER training data.
- Training-data terms that may restrict redistribution: DIV2K (Real-ESRGAN), Places2 (MI-GAN), MS-Celeb-1M (SFace),
  the Nomos images.
- Not verified: SigLIP 2 loading in candle 0.9.2, whether `kitoken` (SentencePiece) is pure Rust, Burn's dependency
  tree, MediaPipe `.task` contents, DA3's architecture.
- Face recognition is biometric data (GDPR Art. 9, Illinois BIPA; not verified here, not legal advice): keep it off by
  default, local, with a "delete all face data" action, and never sync face embeddings without consent.
