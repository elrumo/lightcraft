//! Compact pictures: linear light in half floats ([`Rgb16f`], 6 bytes a pixel instead of 12), for big originals held
//! whole while only part of them is rendered at a time. A reader widens the part it needs to [`Rgb32f`]
//! ([`PixelView::region`], [`resize_view_with`](crate::resample::resize_view_with)); widening is exact, so the
//! floats it gives are the ones a picture widened whole would hold.
//!
//! Half floats keep 11 significant bits (a relative step of 1/2048) from 6.1e-5 to 65504, and less below: an 8-bit
//! render of a picture held this way is the f32 render's within one level (`lightcraft-pipeline`'s
//! `compact_sources_render_like_floats`), raws' highlights above 1 included.

use crate::{Image, Rgb32f, par_rows};
use half::f16;
use std::borrow::Cow;
use std::sync::Arc;

/// Linear RGB in half floats.
pub type Rgb16f = Image<[f16; 3]>;

impl Rgb16f {
    /// `img` in half floats (to nearest; a value past 65504 becomes infinite, as f16 has no larger one).
    pub fn from_f32(img: &Rgb32f) -> Rgb16f {
        img.map(narrow)
    }

    /// The whole picture as floats.
    pub fn widen(&self) -> Rgb32f {
        self.map(widen)
    }

    /// Pixels `x0..x1` of rows `y0..y1` as floats (held to the picture; empty when nothing is left).
    pub fn region(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> Rgb32f {
        let (x1, y1) = (x1.min(self.width), y1.min(self.height));
        let (x0, y0) = (x0.min(x1), y0.min(y1));
        let w = x1 - x0;
        let mut out = Rgb32f::new(w, y1 - y0);
        par_rows(&mut out.data, w, |y, row| {
            let at = (y0 + y) * self.width + x0;
            if let Some(src) = self.data.get(at..at + w) {
                widen_into(src, row);
            }
        });
        out
    }

    /// Bytes the pixels take.
    pub fn bytes(&self) -> usize {
        self.data.len().saturating_mul(std::mem::size_of::<[f16; 3]>())
    }
}

/// A pixel in half floats (to nearest).
#[inline]
pub fn narrow(p: [f32; 3]) -> [f16; 3] {
    p.map(f16::from_f32)
}

/// A pixel in half floats as floats (exact).
#[inline]
pub fn widen(p: [f16; 3]) -> [f32; 3] {
    p.map(f16::to_f32)
}

/// `src` widened into `out` (as many as both hold).
#[inline]
pub(crate) fn widen_into(src: &[[f16; 3]], out: &mut [[f32; 3]]) {
    for (o, s) in out.iter_mut().zip(src) {
        *o = widen(*s);
    }
}

/// A picture as a render reads it: floats, or half floats it widens a part of at a time.
#[derive(Clone, Copy, Debug)]
pub enum PixelView<'a> {
    F32(&'a Rgb32f),
    F16(&'a Rgb16f),
}

impl<'a> PixelView<'a> {
    pub fn width(self) -> usize {
        match self {
            PixelView::F32(i) => i.width,
            PixelView::F16(i) => i.width,
        }
    }

    pub fn height(self) -> usize {
        match self {
            PixelView::F32(i) => i.height,
            PixelView::F16(i) => i.height,
        }
    }

    /// Pixels.
    pub fn len(self) -> usize {
        match self {
            PixelView::F32(i) => i.data.len(),
            PixelView::F16(i) => i.data.len(),
        }
    }

    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// The whole picture as floats: borrowed when it is, else widened (a copy as big as the floats: for readers that
    /// need all of it at once, never kept).
    pub fn to_f32(self) -> Cow<'a, Rgb32f> {
        match self {
            PixelView::F32(i) => Cow::Borrowed(i),
            PixelView::F16(i) => Cow::Owned(i.widen()),
        }
    }

    /// Pixels `x0..x1` of rows `y0..y1` as floats (held to the picture).
    pub fn region(self, x0: usize, y0: usize, x1: usize, y1: usize) -> Rgb32f {
        match self {
            PixelView::F32(i) => {
                let (x1, y1) = (x1.min(i.width), y1.min(i.height));
                let (x0, y0) = (x0.min(x1), y0.min(y1));
                i.crop(x0, y0, x1 - x0, y1 - y0)
            }
            PixelView::F16(i) => i.region(x0, y0, x1, y1),
        }
    }

    /// The pixel at `(x, y)` (`None` outside the picture).
    #[inline]
    pub fn get(self, x: usize, y: usize) -> Option<[f32; 3]> {
        if x >= self.width() {
            return None;
        }
        let i = y.checked_mul(self.width())?.checked_add(x)?;
        match self {
            PixelView::F32(img) => img.data.get(i).copied(),
            PixelView::F16(img) => img.data.get(i).copied().map(widen),
        }
    }

    /// The pixels' address, for telling one picture from another while both are alive.
    pub fn addr(self) -> usize {
        match self {
            PixelView::F32(i) => i.data.as_ptr() as usize,
            PixelView::F16(i) => i.data.as_ptr() as usize,
        }
    }
}

impl<'a> From<&'a Rgb32f> for PixelView<'a> {
    fn from(i: &'a Rgb32f) -> Self {
        PixelView::F32(i)
    }
}

impl<'a> From<&'a Arc<Rgb32f>> for PixelView<'a> {
    fn from(i: &'a Arc<Rgb32f>) -> Self {
        PixelView::F32(i)
    }
}

impl<'a> From<&'a Rgb16f> for PixelView<'a> {
    fn from(i: &'a Rgb16f) -> Self {
        PixelView::F16(i)
    }
}

impl<'a> From<&'a Arc<Rgb16f>> for PixelView<'a> {
    fn from(i: &'a Arc<Rgb16f>) -> Self {
        PixelView::F16(i)
    }
}

impl<'a> From<&'a SourcePixels> for PixelView<'a> {
    fn from(s: &'a SourcePixels) -> Self {
        s.view()
    }
}

/// A shared picture in floats or half floats (what a render's source and its caches hold).
#[derive(Clone, Debug)]
pub enum SourcePixels {
    F32(Arc<Rgb32f>),
    F16(Arc<Rgb16f>),
}

impl SourcePixels {
    pub fn view(&self) -> PixelView<'_> {
        match self {
            SourcePixels::F32(i) => PixelView::F32(i),
            SourcePixels::F16(i) => PixelView::F16(i),
        }
    }

    pub fn width(&self) -> usize {
        self.view().width()
    }

    pub fn height(&self) -> usize {
        self.view().height()
    }

    /// Bytes the pixels take.
    pub fn bytes(&self) -> usize {
        match self {
            SourcePixels::F32(i) => i.data.len().saturating_mul(12),
            SourcePixels::F16(i) => i.bytes(),
        }
    }

    /// The floats themselves, when that is how the picture is held.
    pub fn as_f32(&self) -> Option<&Arc<Rgb32f>> {
        match self {
            SourcePixels::F32(i) => Some(i),
            SourcePixels::F16(_) => None,
        }
    }

    /// The whole picture as floats: shared when it is, else widened (a copy as big as the floats, for a reader that
    /// needs all of it at once).
    pub fn to_f32(&self) -> Arc<Rgb32f> {
        match self {
            SourcePixels::F32(i) => i.clone(),
            SourcePixels::F16(i) => Arc::new(i.widen()),
        }
    }

    /// The whole picture as owned floats: taken out of the share when this is its only holder, else copied (or widened).
    pub fn into_f32(self) -> Rgb32f {
        match self {
            SourcePixels::F32(i) => Arc::try_unwrap(i).unwrap_or_else(|i| (*i).clone()),
            SourcePixels::F16(i) => i.widen(),
        }
    }

    /// Whether both are the same shared picture.
    pub fn ptr_eq(&self, other: &SourcePixels) -> bool {
        match (self, other) {
            (SourcePixels::F32(a), SourcePixels::F32(b)) => Arc::ptr_eq(a, b),
            (SourcePixels::F16(a), SourcePixels::F16(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// The same picture in half floats (shared when it already is).
    pub fn compact(&self) -> SourcePixels {
        match self {
            SourcePixels::F32(i) => SourcePixels::F16(Arc::new(Rgb16f::from_f32(i))),
            SourcePixels::F16(_) => self.clone(),
        }
    }
}

impl From<&SourcePixels> for SourcePixels {
    fn from(s: &SourcePixels) -> Self {
        s.clone()
    }
}

impl From<Arc<Rgb32f>> for SourcePixels {
    fn from(i: Arc<Rgb32f>) -> Self {
        SourcePixels::F32(i)
    }
}

impl From<&Arc<Rgb32f>> for SourcePixels {
    fn from(i: &Arc<Rgb32f>) -> Self {
        SourcePixels::F32(i.clone())
    }
}

impl From<Rgb32f> for SourcePixels {
    fn from(i: Rgb32f) -> Self {
        SourcePixels::F32(Arc::new(i))
    }
}

impl From<Rgb16f> for SourcePixels {
    fn from(i: Rgb16f) -> Self {
        SourcePixels::F16(Arc::new(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats_widen_back_within_their_precision_and_regions_match_the_whole() {
        let img = Rgb32f::from_fn(37, 23, |x, y| [x as f32 / 7.0, (y as f32 - 5.0) / 3.0, 1e-4 * (x * y) as f32 + 2.5]);
        let h = Rgb16f::from_f32(&img);
        assert_eq!(h.bytes(), 37 * 23 * 6);
        let back = h.widen();
        for (a, b) in img.data.iter().zip(&back.data) {
            for c in 0..3 {
                assert!((a[c] - b[c]).abs() <= a[c].abs() / 2048.0 + 1e-7, "{a:?} {b:?}");
            }
        }
        // a region is the widened whole's crop, held to the picture
        assert_eq!(h.region(5, 3, 20, 11).data, back.crop(5, 3, 15, 8).data);
        assert_eq!(h.region(30, 20, 99, 99).data, back.crop(30, 20, 7, 3).data);
        assert!(h.region(50, 50, 60, 60).is_empty());
        let v = PixelView::from(&h);
        assert_eq!(v.region(1, 2, 4, 6).data, back.crop(1, 2, 3, 4).data);
        assert_eq!(PixelView::from(&back).region(1, 2, 4, 6).data, back.crop(1, 2, 3, 4).data);
        assert_eq!(v.get(36, 22), Some(back.get(36, 22)));
        assert_eq!(v.get(37, 0), None);
        assert_eq!(v.get(0, 23), None);
        let s = SourcePixels::from(img.clone()).compact();
        assert_eq!(s.to_f32().data, back.data);
        assert!(s.ptr_eq(&s.clone()) && !s.ptr_eq(&s.compact().view().to_f32().into_owned().into()));
    }
}
