//! Joint colour noise reduction on the device (`lightcraft_pipeline::nrc_joint`, process V7 on): the kernels of
//! `wgsl/nrcj.wgsl`, run in the order of `nrc_joint::denoise_with` at step 1 (the only step a render uses).
//! Every number a decision rests on comes from the CPU module (`scale_plan`, `strength`, `tolerance`,
//! `speck_brake`, the noise model's fit), so the two take the same steps; the CPU stage stays the reference and
//! the fallback ([`denoise`] answers `None` where it can't run).

use lightcraft_pipeline::local::NrcParams;
use lightcraft_pipeline::nrc_joint::{self, JointNoise};

use crate::ctx::{Buf, groups1};
use crate::render::{Cx, gaussian};

/// Values the `nrcj_samples` kernel writes per noise sample: valid, brightness, then the squared residuals of
/// Y, R−Y, B−Y at spacings 1, 2 and 4.
const SAMPLE_WORDS: usize = 11;

/// `LIGHTCRAFT_NR_JOINT_CPU=1`: the joint colour noise reduction runs on the CPU inside GPU renders, as it did
/// before it had kernels (to compare the two, or to rule the kernels out).
fn forced_cpu() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("LIGHTCRAFT_NR_JOINT_CPU").is_ok_and(|v| matches!(v.trim(), "1" | "on" | "true" | "yes")))
}

/// The noise model measured on the `w × h` linear RGB image `img` itself (`nrc_joint::noise_model`: the kernel takes
/// the samples, the CPU fits them). `None` for an image too small to measure, which the CPU stage handles.
fn measure(cx: &mut Cx<'_>, img: &Buf, w: usize, h: usize) -> Option<JointNoise> {
    if w < 3 || h < 3 {
        return None;
    }
    let stride = nrc_joint::noise_sample_stride(w * h);
    let count = (w * h).div_ceil(stride);
    let out = cx.gpu.buffer(count * SAMPLE_WORDS);
    cx.run("nrcj_samples", &[count as u32, w as u32, h as u32, stride as u32], &[Some(img), None, Some(&out)], groups1(count));
    let words: Vec<f32> = cx.read(&out, count * SAMPLE_WORDS);
    // (the CPU drops a sample whose pixels are not finite: such a pixel makes some of the values non-finite)
    let (records, _) = words.as_chunks::<SAMPLE_WORDS>();
    let samples = records
        .iter()
        .filter(|c| c[0] != 0.0 && c.iter().all(|v| v.is_finite()))
        .map(|c| (c[1], [[c[2], c[3], c[4]], [c[5], c[6], c[7]], [c[8], c[9], c[10]]]))
        .collect();
    Some(nrc_joint::noise_from_samples(samples))
}

/// The leading parameters every kernel with the noise model takes: n, w, h, alpha, beta, ymax, tolerance
/// (`P[0..13]` of `nrcj.wgsl`).
fn noise_words(n: usize, w: usize, h: usize, noise: &JointNoise) -> Vec<u32> {
    let mut p = vec![n as u32, w as u32, h as u32];
    p.extend(noise.model.alpha.iter().chain(&noise.model.beta).chain(&noise.ymax).chain([&noise.tolerance]).map(|v| v.to_bits()));
    p
}

/// `nrc_joint::denoise` of the `w × h` linear RGB image `img` (Color `amount` and Color Detail `detail`, 0..=1;
/// `noise`: the whole frame's noise model, `None` measures it on the image), as a new buffer. `None`: not done here,
/// the caller runs the CPU stage (an image too small to measure, or `LIGHTCRAFT_NR_JOINT_CPU`).
pub(crate) fn denoise(
    cx: &mut Cx<'_>,
    img: &Buf,
    (w, h): (usize, usize),
    params: &NrcParams,
    detail: f32,
    amount: f32,
    noise: Option<&JointNoise>,
) -> Option<Buf> {
    let n = w.checked_mul(h).filter(|n| n.checked_mul(3).is_some_and(|words| u32::try_from(words).is_ok()))?;
    let strength = nrc_joint::strength(amount);
    if n == 0 || strength <= 0.0 || forced_cpu() {
        return None;
    }
    let mut noise = match noise {
        Some(m) => *m,
        None => measure(cx, img, w, h)?,
    };
    noise.tolerance = nrc_joint::tolerance(detail);
    let brake = nrc_joint::speck_brake(amount);
    let np = noise_words(n, w, h, &noise);
    let g = groups1(n);
    // Every plane is a buffer of 3 values a pixel, even the pairs (2 used): the pool hands a freed buffer only to a
    // request of about its size, so planes of one size reuse each other's (peak device memory: 6 planes of 3 values
    // a pixel, about 0.9 GB at 12 MP, where sizes of 2 and 3 apart took 9).
    let plane = |cx: &mut Cx<'_>| cx.gpu.buffer(3 * n);

    // Y, R−Y, B−Y; the speck brake changes the chroma of compact specks (before the guide is built)
    let mut t = plane(cx);
    cx.run("nrcj_yuv", &[n as u32, 0], &[Some(img), None, Some(&t)], g);
    if brake.share > 0.0 && w >= 5 && h >= 5 {
        let braked = plane(cx);
        let mut p = np.clone();
        p.extend([brake.threshold.to_bits(), brake.share.to_bits()]);
        cx.run("nrcj_speck", &p, &[Some(&t), None, Some(&braked)], g);
        t = braked;
        cx.flush(); // (the transformed image goes back to the pool)
    }
    // the guide (a pilot: the lightly blurred image, along the smoother axis at strong edges), and the chroma pairs
    // that are filtered
    let blur = gaussian(cx, &t, w, h, 3, 0.8);
    cx.flush(); // (the blur's second buffer)
    let guide = plane(cx);
    cx.run("nrcj_pilot", &np, &[Some(&t), Some(&blur), Some(&guide)], g);
    let mut filtered = plane(cx);
    cx.run("nrcj_uv", &[n as u32], &[Some(&t), None, Some(&filtered)], g);
    drop((t, blur));
    cx.flush();

    // exchanges at doubling distances. `paths`: exp(−largest guide distance along the path) to the neighbour
    // `delta` away; the first scale's pair weights are its paths.
    let mut paths = plane(cx);
    cx.run("nrcj_links1", &np, &[Some(&guide), None, Some(&paths)], g);
    let mut links: Option<Buf> = None;
    let mut scratch = plane(cx);
    let plan = nrc_joint::scale_plan(params.passes.last().map_or(0.0, |p| p.sigma), w, h);
    for (k, &(delta, rate)) in plan.iter().enumerate() {
        let weights = links.as_ref().unwrap_or(&paths);
        let p = [n as u32, w as u32, h as u32, delta as u32, rate.to_bits()];
        for _ in 0..2 {
            cx.run("nrcj_dh", &p, &[Some(&filtered), Some(weights), Some(&scratch)], g);
            std::mem::swap(&mut filtered, &mut scratch);
            cx.run("nrcj_dv", &p, &[Some(&filtered), Some(weights), Some(&scratch)], g);
            std::mem::swap(&mut filtered, &mut scratch);
        }
        let Some(&(next, _)) = plan.get(k + 1) else { break };
        // paths of twice the length (into the idle scratch plane), and the next scale's weights from them
        drop(links.take());
        cx.run("nrcj_paths", &[n as u32, w as u32, h as u32, delta as u32], &[Some(&paths), None, Some(&scratch)], g);
        std::mem::swap(&mut paths, &mut scratch);
        cx.flush(); // (the old weights go back to the pool)
        let next_links = plane(cx);
        let mut p = np.clone();
        p.push(next as u32);
        cx.run("nrcj_links", &p, &[Some(&guide), Some(&paths), Some(&next_links)], g);
        links = Some(next_links);
    }
    drop((guide, paths, links, scratch));

    // back to RGB with the image's own luminance, blended by the strength
    let out = plane(cx);
    cx.run("nrcj_recon", &[n as u32, strength.to_bits(), 0, 0], &[Some(img), Some(&filtered), Some(&out)], g);
    Some(out)
}
