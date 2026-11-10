use super::encoding::srgb8_in;
use super::options::MAX_ENLARGED_PIXELS;
use super::*;

/// A plain frame around an exported photo: the picture grows by the border on every side, so the photo keeps the size it
/// was exported at. Drawn last, after output sharpening and the watermark (which stay on the photo).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Border {
    /// Each side's width as a fraction of the photo's short edge ([`BORDER_WIDTH_RANGE`]), so every export size looks the same.
    pub width: f32,
    /// sRGB colour.
    pub color: [u8; 3],
}

impl Default for Border {
    fn default() -> Self {
        Self { width: 0.05, color: [255; 3] }
    }
}

/// The keys of a `border` object ([`Border`], camelCase).
pub const BORDER_PARAMS: &[&str] = &["width", "color"];

/// A side's width as a fraction of the short edge.
pub const BORDER_WIDTH_RANGE: (f32, f32) = (0.0, 0.25);

impl Border {
    /// Each side's width in pixels around a `w × h` photo: at least one when a width is asked for, none for 0 (or a width
    /// that is not a number).
    pub fn px(&self, w: usize, h: usize) -> usize {
        let f = if self.width.is_finite() { self.width.clamp(BORDER_WIDTH_RANGE.0, BORDER_WIDTH_RANGE.1) } else { 0.0 };
        if f <= 0.0 || w == 0 || h == 0 {
            return 0;
        }
        ((w.min(h) as f32 * f).round() as usize).max(1)
    }
}

/// `w × h` grown by `side` on every side, or an error when the picture would be too big to make.
fn grown(w: usize, h: usize, side: usize) -> Result<(usize, usize), String> {
    let edge = |n: usize| side.checked_mul(2).and_then(|s| n.checked_add(s));
    match (edge(w), edge(h)) {
        (Some(nw), Some(nh)) if nw.checked_mul(nh).is_some_and(|n| n as f64 <= MAX_ENLARGED_PIXELS) => Ok((nw, nh)),
        _ => Err("the photo with its border is too big to export: choose a smaller size or a thinner border".into()),
    }
}

/// `src` (`w` pixels of `fill.len()` samples a row) in the middle of an `nw × nh` picture of `fill`, `side` pixels from each edge.
fn framed<T: Copy>(src: &[T], w: usize, nw: usize, nh: usize, side: usize, fill: &[T]) -> Vec<T> {
    let ch = fill.len();
    let mut out: Vec<T> = fill.iter().copied().cycle().take(nw.saturating_mul(nh).saturating_mul(ch)).collect();
    let row = w.saturating_mul(ch);
    if row == 0 {
        return out;
    }
    for (y, line) in src.chunks_exact(row).enumerate() {
        let at = ((y + side) * nw + side) * ch;
        if let Some(dst) = out.get_mut(at..at + row) {
            dst.copy_from_slice(line);
        }
    }
    out
}

/// `img` (in `space`) with `b` drawn around it.
pub fn add_border(img: Rgba8, b: &Border, space: OutputSpace) -> Result<Rgba8, String> {
    let side = b.px(img.width, img.height);
    if side == 0 {
        return Ok(img);
    }
    let (nw, nh) = grown(img.width, img.height, side)?;
    let c = srgb8_in(space, b.color);
    let data = framed(&img.data, img.width, nw, nh, side, &[[c[0], c[1], c[2], 255]]);
    Ok(Rgba8 { width: nw, height: nh, data })
}

/// [`add_border`] on a high-bit-depth image: 16-bit samples are encoded with the space's curve, float ones are linear (as the
/// watermark's colour is drawn into them).
pub fn add_border_deep(img: DeepImage, b: &Border) -> Result<DeepImage, String> {
    let side = b.px(img.width, img.height);
    if side == 0 {
        return Ok(img);
    }
    let (nw, nh) = grown(img.width, img.height, side)?;
    let (w, space) = (img.width, img.space);
    let c = srgb8_in(space, b.color);
    let samples = match img.samples {
        DeepSamples::U16(v) => DeepSamples::U16(framed(&v, w, nw, nh, side, &c.map(|c| u16::from(c) * 257))),
        DeepSamples::F32(v) => {
            let trc = space.trc();
            DeepSamples::F32(framed(&v, w, nw, nh, side, &c.map(|c| trc.decode(f32::from(c) / 255.0))))
        }
    };
    Ok(DeepImage { width: nw, height: nh, space, samples })
}
