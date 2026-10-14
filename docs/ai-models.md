# Optional AI models

LightCraft never bundles model weights. Each model is **optional**: it is downloaded only when the user asks, after
the app names its licence, and every file is verified (exact size and SHA-256 where pinned) before use. Without a
model, the feature that needs it says so and everything else works. Results are baked (SAM 3 stores its mask
logits; denoise, super resolution and depth will store an enhanced image or a depth map), so renders and exports never
run a model.

`crates/fetch` is the downloader (ordered mirrors, resume, `.part` files, size cap, cancel, pinned SHA-256, pure-Rust
HTTPS) and the engine's `Downloader` runs it on a background thread. What to download belongs to the feature that
needs the model: `crates/segment` (SAM 3), `crates/enhance` (`models::NOMOS_SPAN_2X`: pinned size and SHA-256,
licence, credit), `crates/vision` (SigLIP 2, PP-OCRv6, YuNet + SFace). Inference lives in the same crates.
`crates/engine/src/models.rs` lists them all as one.

Models whose licence allows redistribution (Apache-2.0, MIT, BSD, CC-BY) can be mirrored by LightCraft with their
licence text and attribution next to them; gated or custom-licence models (SAM 3) cannot, and need the user's own
mirror or a manual install ([ai-masks.md](ai-masks.md)).

Today: **SAM 3** (Object and Describe masks, [ai-masks.md](ai-masks.md)), **Nomos Uni SPAN 2×** (Super
Resolution, [super-resolution.md](super-resolution.md)), **MI-GAN** (AI Remove, [ai-remove.md](ai-remove.md), and
Generative Expand: the one model that also runs in the browser, on rten, from `crates/erase`) and **LaMa** (Generative
Expand on computers, [generative-expand.md](generative-expand.md), also on rten). The rest of this page is the research behind what to add next
(2026-10-08; read from each repo's LICENSE and each Hugging Face model card — **re-check the licence of a model when
you add it**, and write the verdict and the URL you read in its `ModelSpec`).

Models that are public and ungated (Nomos) are downloaded from their author's own repository, pinned by size and
SHA-256: LightCraft hosts nothing and redistributes nothing. Only a gated model (SAM 3) needs a mirror of our own.

## Managing models

**Settings → AI Models** lists every model in one place. For each: what it is for, whether it is **on this device**
(and the space it takes, with its folder and Show in Folder) or **not downloaded** (its size, and the host it would
come from), its licence and credit, **Use this model** (turn it off and its feature says so and does nothing; the files
stay), **Download…** (asks first, naming the size, the host and the licence), **Cancel Download**, and **Delete…**
(asks first; removes only the model's own files, never anything else in the folder; it can be downloaded again). A
model this build can't run (SAM 3 on an iPhone) is listed as not available. The first time a feature needs its model
it offers the download itself (Object / Describe masks, Super Resolution). The on / off choice is saved with the app
settings (`models_disabled` in `ui.json`; the SwiftUI app keeps it as `modelsDisabled` in its own settings and gives it back to the engine at launch).

Agents get the same through commands: `models.list`, `models.download {id, acknowledged: true}`, `models.cancel {id}`,
`models.delete {id, confirm: true}`, `models.setEnabled {id, enabled}`.

## Apple's models (SwiftUI iOS app)

Apple ships models the app can use without downloading anything of its own: Vision's foreground
instance mask and, on iOS 27, tap-to-segment (include / exclude taps, a model iOS downloads on
request). They back Select Subject and Select Object in the SwiftUI app ([ai-masks.md](ai-masks.md)),
and Settings ▸ AI Models ▸ Selection chooses Automatic, Apple or Classic. They are Apple's models
under Apple's terms and are never mirrored or bundled by LightCraft.

What Apple does **not** offer (checked in the iOS 27.1 SDK, 2026-10-14): any erase or inpainting
API (Photos' Clean Up is private), any out-painting API, a sky model. Image Playground's sheet
(`imagePlaygroundSheet`, `CreationStrategy.editExisting`) runs in Apple's cloud (Private Cloud
Compute), takes no mask and returns a flat picture; the programmatic `ImageCreator` is removed in
iOS 27. AI Remove and Generative Expand therefore keep MI-GAN, LaMa and Classic.
Using the Image Playground sheet as an extra, opt-in Generative Expand engine was considered and not built: it needs an Apple
Intelligence device and Apple's sheet (not scriptable, absent from simulators), sends the photo to Apple's cloud, and
may re-render the whole picture rather than extend it.

## Core ML spike (2026-10-15)

Question: can the models LightCraft already uses (MI-GAN, LaMa) run on Apple's GPU or Neural Engine, so that AI Remove
and Generative Expand are fast on an iPhone, where LaMa is too slow on the CPU today? `tools/coreml_convert.py`
converts the pinned ONNX files (the ones `crates/erase` downloads) to Core ML (ML Program, fp16, iOS 17), checks them
against onnxruntime and times them on an Apple silicon Mac, on a 512 x 512 crop with an ellipse hole (best of three
runs after the first). The Mac was busy (load average in the hundreds) and **no iPhone was used**: read the ratios, not
the seconds.

| Model | Core ML size (ONNX) | CPU | CPU + GPU | CPU + Neural Engine | rten on the CPU |
|---|---|---|---|---|---|
| MI-GAN | 13.5 MB (29.5 MB) | 0.78 s | **0.022 s** | 0.039 s, **wrong answers** | about 0.35 s |
| LaMa | 98.7 MB (208 MB) | 7.8 s | **0.146 s** | 9.1 s | 7 to 11 s |

Against onnxruntime on the GPU path the answers differ by at most 1.7 (MI-GAN) and 4.0 (LaMa) levels of 255, 0.012 and
0.027 on average. Findings:
- **The GPU is the target** (`MLModelConfiguration.computeUnits = .cpuAndGPU`): 16x faster than the CPU for MI-GAN and
  50x for LaMa. Never let MI-GAN reach the Neural Engine (`.all`, `.cpuAndNeuralEngine`): its answer is wrong (errors up
  to 255, most likely fp16 range). LaMa's Neural Engine path ran at CPU speed, and `.all` specialized for 253 s on first
  load (the GPU path took 11 s); Core ML caches the result.
- **LaMa becomes possible on a phone**: 99 MB instead of 208 MB, about 0.15 s on a Mac GPU. An iPhone's GPU is slower
  and its memory tighter: measure there before promising it.
- **Conversion needs two fixes**, both in the script: fixed 512 x 512 shapes folded by onnx-simplifier (LaMa's graph
  is 17,480 nodes of FFT arithmetic, 3,028 after), and LaMa's reverse slices (`x[1:-1][::-1]`, the inverse FFT) rewritten
  as constant `Gather`s because Core ML mis-infers a negative-step slice's shape. MI-GAN's published file takes and
  answers uint8, so its uint8 casts become identities, as `vision::onnxfix` does for rten. The conversion goes through
  onnx2torch (MI-GAN's PyTorch weights are only on Google Drive; the ONNX files are the pinned, verified source).
- **Not done**: an iPhone measurement; the Rust side (an `objc2-core-ml` crate behind the `Painter` trait, `unsafe` only
  in it, `EraseModel` holding an `Arc<dyn Painter>`); where the converted packages are hosted (an `.mlpackage` is a
  folder, so a zip; the code and weights are MIT and Apache-2.0, which allow a mirror with their licence text; the
  Places2 terms are still unverified; LightCraft hosts nothing today).

Verdict: **go** for Core ML on the GPU as a faster backend for both models (and LaMa on iPhone), after an iPhone
timing. Not scheduled.

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
| Remove (large areas) | MI-GAN | MIT / MIT | 29.5 MB | **in use** (AI Remove, rten; `migan.onnx` from the authors' Hugging Face repo, MIT); separable convs, no FFT; Places2/FFHQ terms unverified |
| Generative expand, large holes | LaMa (big-lama; fp32 ONNX export `Carve/LaMa-ONNX`) | Apache-2.0 / Apache-2.0 | 208 MB | **in use** (Generative Expand on computers, `crates/erase/src/lama.rs`; `lama_fp32.onnx` at a pinned revision, size and SHA-256; its answers on rten match onnxruntime's to 2e-6, checked 2026-10-13); runs on rten as is (its FFTs are MatMul / Einsum / Cos / Sin), checked 2026-10-12: right on a synthetic hole, 7–11 s per 512 × 512 pass on a busy Apple-silicon CPU (MI-GAN: ~1 s), so desktop only; on a square crop widened back to 3:2 it continued sky, sea, bushes and a film border plausibly but softly, a little cleaner than MI-GAN; Places2 terms unverified |

Subject, Sky, People and Landscape masks can use SAM 3's Describe path with fixed prompts for users who have it.

**Generative expand (outpainting), checked 2026-10-12.** Nothing permissively licensed reaches Lightroom's (diffusion)
quality at a size a laptop, phone or browser can run. MI-GAN and LaMa (above) extend simple surroundings (sky, sea,
foliage, texture) at 512 × 512 over the whole canvas, so the new strip is soft and can't hold new objects. Diffusion
inpainting: Kandinsky 2.2 inpaint is Apache-2.0 but a 15 GB repository with no pure-Rust implementation;
Stable Diffusion inpainting (CreativeML OpenRAIL-M) and SDXL inpainting (OpenRAIL++) carry use-based restrictions;
FLUX.1 Fill is non-commercial. Classical fallback already in the code: the panorama's content-aware Fill Edges.
**Built 2026-10-13** ([generative-expand.md](generative-expand.md)): MI-GAN on every device, LaMa on computers,
and Classic (diffused edge colours with synthesized texture) without a download; one whole-canvas pass for the
structure, the photo's grain added by patch synthesis. Windows repainted at full scale were tried and dropped (streaks
far from the photo).

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
