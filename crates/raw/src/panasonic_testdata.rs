//! Synthetic Panasonic files for other crates' tests (`lightcraft_raw::testdata::PanasonicRw2`): an RW2 container (a TIFF
//! with the `U` magic) of 12-bit samples stored as 16-bit words, with the sensor size, the borders and, when asked, the
//! in-camera aspect-ratio crop in IFD0. The layout is the one `vendor::rw2` reads from the real files.

use lightcraft_tiff::{ByteOrder, IfdBuilder, TiffWriter, Value, tags as t};

/// A synthetic Panasonic file. [`Self::new`] gives one with no aspect-ratio crop: fill the public fields for the rest,
/// then [`Self::build`].
#[derive(Clone, Debug)]
pub struct PanasonicRw2 {
    /// The sensor's size, borders included.
    pub width: u16,
    pub height: u16,
    /// Rows and columns of masked border at the top and left (the bottom and right have none): the active area is the
    /// rest.
    pub border: (u16, u16),
    /// `0x002f` to `0x0032`: the frame the in-camera aspect ratio cuts, as top, left, bottom and right in sensor
    /// coordinates.
    pub crop: Option<[u16; 4]>,
    /// The container's Exif orientation (1 to 8).
    pub orientation: u16,
    /// Exif model (the make is `Panasonic`).
    pub model: String,
    /// The sample of every pixel.
    pub level: u16,
}

impl PanasonicRw2 {
    /// A `width × height` sensor with a border of 2 rows at the top and 4 columns at the left.
    pub fn new(width: u16, height: u16) -> Self {
        PanasonicRw2 { width, height, border: (2, 4), crop: None, orientation: 1, model: "DC-GH7".into(), level: 1500 }
    }

    /// The file's bytes (empty if the TIFF writer refused the container, which a test sees as an unreadable file).
    pub fn build(&self) -> Vec<u8> {
        let samples = usize::from(self.width) * usize::from(self.height);
        // 12-bit samples left-justified in 16-bit words, little-endian (compression 34828: the oldest files' layout)
        let payload: Vec<u8> = std::iter::repeat_n((self.level << 4).to_le_bytes(), samples).flatten().collect();
        let build = |off: u32| {
            let mut ifd = IfdBuilder::new();
            let (top, left) = self.border;
            for (tag, v) in [(0x0002, self.width), (0x0003, self.height), (0x0004, top), (0x0005, left), (0x0006, self.height), (0x0007, self.width)]
            {
                ifd.set(tag, Value::Short(vec![v]));
            }
            // 12 bits, compression 34828, an RGGB mosaic
            for (tag, v) in [(0x000a, 12), (0x000b, 34828), (0x0009, 1)] {
                ifd.set(tag, Value::Short(vec![v]));
            }
            if let Some(frame) = self.crop {
                for (tag, v) in [0x002f, 0x0030, 0x0031, 0x0032].into_iter().zip(frame) {
                    ifd.set(tag, Value::Short(vec![v]));
                }
            }
            ifd.set(t::MAKE, Value::Ascii("Panasonic".into()));
            ifd.set(t::MODEL, Value::Ascii(self.model.clone()));
            ifd.set(t::ORIENTATION, Value::Short(vec![self.orientation]));
            ifd.set(t::STRIP_OFFSETS, Value::Long(vec![off]));
            let mut bytes = TiffWriter::new(ByteOrder::Little, false).write(&[ifd]).unwrap_or_default();
            if let Some(magic) = bytes.get_mut(2) {
                *magic = 0x55;
            }
            bytes
        };
        // (the payload follows the directory, on a 16-byte boundary: its offset is in the directory, whose size it doesn't change)
        let off = build(0).len().div_ceil(16) * 16;
        let mut bytes = build(u32::try_from(off).unwrap_or_default());
        bytes.resize(off, 0);
        bytes.extend_from_slice(&payload);
        bytes
    }
}
