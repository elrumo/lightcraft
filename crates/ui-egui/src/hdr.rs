//! HDR pictures in the window (LR-VIEW-HDR-DISPLAY): where the loupe's HDR render lives on the GPU.
//!
//! The scheduler renders an HDR edit's float picture when the display can show the range above
//! SDR white ([`lightcraft_apphost::render::HdrPicture`]); the window's renderer, which owns the
//! GPU device, turns it into a texture egui can draw. That renderer is the desktop app
//! (`apps/lightcraft`, through [`HdrSink`]); a window without one (the headless UI, the web) keeps
//! showing the SDR rendition.

use std::sync::Arc;

pub use lightcraft_apphost::render::HdrPicture;

/// Makes and frees the textures of HDR pictures: `Rgba16Float`, as they are (encoded with the sRGB
/// curve, SDR white at 1.0). egui draws one like any other texture: its textures aren't sRGB-aware,
/// it writes what it samples, so values above 1 reach a surface that can show them.
pub trait HdrSink: Send + Sync {
    /// A texture of `picture`. `reuse`: the texture made for this slot's previous picture, to write
    /// into when it is the same size (the id stays the same). `None`: the device refused.
    fn upload(&self, picture: &HdrPicture, reuse: Option<egui::TextureId>) -> Option<egui::TextureId>;
    /// The texture of an id [`Self::upload`] returned is not needed any more.
    fn free(&self, id: egui::TextureId);
}

/// A slot's HDR picture as a texture egui draws instead of its 8-bit one; freed when it goes.
pub struct HdrTex {
    pub id: egui::TextureId,
    pub size: [usize; 2],
    sink: Arc<dyn HdrSink>,
}

impl HdrTex {
    /// `picture` as a texture, or `None` when the sink can't make one. Writes into `reuse`'s
    /// texture when it is the same size.
    pub(crate) fn make(sink: &Arc<dyn HdrSink>, picture: &HdrPicture, reuse: Option<HdrTex>) -> Option<HdrTex> {
        let size = [picture.width, picture.height];
        let reusable = reuse.filter(|r| r.size == size);
        match sink.upload(picture, reusable.as_ref().map(|r| r.id)) {
            Some(id) if reusable.as_ref().is_some_and(|r| r.id == id) => reusable,
            Some(id) => Some(HdrTex { id, size, sink: sink.clone() }),
            None => None,
        }
    }
}

impl Drop for HdrTex {
    fn drop(&mut self) {
        self.sink.free(self.id);
    }
}

/// A sink that makes no texture but hands out ids and remembers what happened to them (the tests').
#[cfg(test)]
pub(crate) mod recorder {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    pub(crate) struct Recorder {
        pub log: Mutex<Vec<String>>,
        pub next: Mutex<u64>,
        pub refuse: Mutex<bool>,
    }

    impl HdrSink for Recorder {
        fn upload(&self, p: &HdrPicture, reuse: Option<egui::TextureId>) -> Option<egui::TextureId> {
            if *self.refuse.lock().unwrap() {
                return None;
            }
            if let Some(id) = reuse {
                self.log.lock().unwrap().push(format!("write {id:?} {}x{}", p.width, p.height));
                return Some(id);
            }
            let mut n = self.next.lock().unwrap();
            *n += 1;
            self.log.lock().unwrap().push(format!("new {} {}x{}", *n, p.width, p.height));
            Some(egui::TextureId::User(*n))
        }

        fn free(&self, id: egui::TextureId) {
            self.log.lock().unwrap().push(format!("free {id:?}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::recorder::Recorder;
    use super::*;

    fn picture(w: usize, h: usize) -> HdrPicture {
        HdrPicture { width: w, height: h, rgba16f: vec![0; w * h * 4], peak: 8.0 }
    }

    #[test]
    fn a_texture_is_reused_for_a_picture_of_its_size_and_freed_when_it_goes() {
        let rec = Arc::new(Recorder::default());
        let sink: Arc<dyn HdrSink> = rec.clone();
        let a = HdrTex::make(&sink, &picture(4, 2), None).unwrap();
        let id = a.id;
        // the next picture of the same size is written into the same texture
        let b = HdrTex::make(&sink, &picture(4, 2), Some(a)).unwrap();
        assert_eq!(b.id, id);
        // another size: the old texture is freed first, and a new one made
        let c = HdrTex::make(&sink, &picture(8, 2), Some(b)).unwrap();
        assert_ne!(c.id, id);
        drop(c);
        assert_eq!(
            *rec.log.lock().unwrap(),
            vec!["new 1 4x2".to_string(), "write User(1) 4x2".into(), "free User(1)".into(), "new 2 8x2".into(), "free User(2)".into()]
        );
    }

    #[test]
    fn a_device_that_refuses_leaves_no_texture() {
        let rec = Arc::new(Recorder::default());
        let sink: Arc<dyn HdrSink> = rec.clone();
        let a = HdrTex::make(&sink, &picture(2, 2), None).unwrap();
        *rec.refuse.lock().unwrap() = true;
        assert!(HdrTex::make(&sink, &picture(2, 2), Some(a)).is_none());
        assert_eq!(rec.log.lock().unwrap().last().map(String::as_str), Some("free User(1)"), "the old texture still goes");
    }
}
