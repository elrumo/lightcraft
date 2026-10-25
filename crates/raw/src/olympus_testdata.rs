//! Synthetic Olympus / OM System files for other crates' tests (`lightcraft_raw::testdata::OlympusOrf`): an ORF
//! container (`IIRO`: a TIFF with a different magic) whose new-style maker note (`OLYMPUS\0II\x03\0`, offsets counted
//! from the note's start) names a preview JPEG in its CameraSettings directory (`0x2020`) and, when asked, carries an
//! ImageProcessing directory (`0x2040`) with the valid area, the aspect frame and the white-balance levels. The layout
//! is the one `vendor::orf` reads from the real files.

use lightcraft_tiff::{ByteOrder, IfdBuilder, ImageData, TiffWriter, Value, tags as t};

/// The twelve temperatures (K) of ImageProcessing's `WB_RBLevels<K>` entries `0x0102` to `0x010d`, in tag order.
pub const PRESET_KELVIN: [u16; 12] = [3000, 3300, 3600, 3900, 4000, 4300, 4500, 4800, 5300, 6000, 6600, 7500];

/// A synthetic Olympus file. [`Self::new`] gives the plain one (no ImageProcessing directory): fill the public fields
/// for the rest, then [`Self::build`].
#[derive(Clone, Debug)]
pub struct OlympusOrf {
    pub width: u32,
    pub height: u32,
    /// Bits per sample stated by the container.
    pub bits: u16,
    /// The image's only strip (compression 1, whatever it holds).
    pub strip: Vec<u8>,
    /// The container's Exif orientation (1 to 8).
    pub orientation: u16,
    /// The camera's preview JPEG, named by CameraSettings.
    pub preview: Vec<u8>,
    /// Exif model (the make is `OLYMPUS IMAGING CORP.`).
    pub model: String,
    /// ImageProcessing `AspectFrame`: left, top, right and bottom (inclusive) pixel of the frame the aspect-ratio setting cuts.
    pub frame: Option<[u16; 4]>,
    /// ImageProcessing `WB_RBLevels`: the as-shot red and blue gains against green, 256 meaning 1.0.
    pub as_shot: Option<[u16; 2]>,
    /// ImageProcessing `WB_RBLevels<K>` ([`PRESET_KELVIN`] in order): the red and blue gains at each stated temperature.
    pub presets: Option<[[u16; 2]; 12]>,
}

impl OlympusOrf {
    /// A `width × height` sensor of `bits`-bit samples whose only strip is `strip`, with `preview` as its camera JPEG.
    pub fn new(width: u32, height: u32, bits: u16, strip: Vec<u8>, preview: Vec<u8>) -> Self {
        OlympusOrf { width, height, bits, strip, orientation: 1, preview, model: "E-M10 Mark II".into(), frame: None, as_shot: None, presets: None }
    }

    /// The file's bytes (empty if the TIFF writer refused the container, which a test sees as an unreadable file).
    pub fn build(&self) -> Vec<u8> {
        let mut ifd = IfdBuilder::new();
        ifd.set(t::IMAGE_WIDTH, Value::Long(vec![self.width]));
        ifd.set(t::IMAGE_LENGTH, Value::Long(vec![self.height]));
        ifd.set(t::BITS_PER_SAMPLE, Value::Short(vec![self.bits]));
        ifd.set(t::COMPRESSION, Value::Short(vec![1]));
        ifd.set(t::PHOTOMETRIC, Value::Short(vec![1]));
        ifd.set(t::MAKE, Value::Ascii("OLYMPUS IMAGING CORP.".into()));
        ifd.set(t::MODEL, Value::Ascii(self.model.clone()));
        ifd.set(t::ORIENTATION, Value::Short(vec![self.orientation]));
        ifd.set_image(ImageData::Strips { rows_per_strip: self.height, strips: vec![self.strip.clone()] });
        let mut exif = IfdBuilder::new();
        // ISO 200, 1/125 s and an RGGB colour-filter layout
        exif.set(t::EXPOSURE_TIME, Value::Rational(vec![(1, 125)]));
        exif.set(t::ISO_SPEED, Value::Short(vec![200]));
        exif.set(0xa302, Value::Undefined(vec![0, 2, 0, 2, 0, 1, 1, 2]));
        exif.set(t::MAKER_NOTE, Value::Undefined(self.maker_note()));
        ifd.set_child(t::EXIF_IFD, exif);
        let mut bytes = TiffWriter::new(ByteOrder::Little, false).write(&[ifd]).unwrap_or_default();
        if let Some(magic) = bytes.get_mut(2..4) {
            magic.copy_from_slice(b"RO");
        }
        bytes
    }

    fn maker_note(&self) -> Vec<u8> {
        let mut processing: Vec<Entry> = Vec::new();
        if self.frame.is_some() || self.as_shot.is_some() || self.presets.is_some() {
            // the valid area is the whole sensor
            for (tag, v) in [(0x0612, 0), (0x0613, 0), (0x0614, self.width), (0x0615, self.height)] {
                processing.push(Entry::long(tag, v));
            }
        }
        if let Some(f) = self.frame {
            processing.push(Entry::shorts(0x1113, &f));
        }
        if let Some([red, blue]) = self.as_shot {
            processing.push(Entry::shorts(0x0100, &[red, blue, 256, 256]));
        }
        for (slot, [red, blue]) in self.presets.iter().flatten().enumerate() {
            processing.push(Entry::shorts(0x0102 + slot as u16, &[*red, *blue, 256, 256]));
        }
        processing.sort_by_key(|e| e.tag);
        let directories = if processing.is_empty() { 1 } else { 2 };
        let camera_settings_at = 12 + 2 + 12 * directories + 4;
        let camera_settings_len = 2 + 2 * 12 + 4;
        let processing_at = camera_settings_at + camera_settings_len;
        let processing_bytes = if processing.is_empty() { Vec::new() } else { directory(processing_at, &processing) };
        let preview_at = processing_at + processing_bytes.len();
        let mut note = b"OLYMPUS\0II\x03\0".to_vec();
        let mut main = vec![Entry::offset(0x2020, camera_settings_at)];
        if !processing.is_empty() {
            main.push(Entry::offset(0x2040, processing_at));
        }
        note.extend(directory(note.len(), &main));
        let camera_settings = [Entry::long(0x0101, preview_at as u32), Entry::long(0x0102, self.preview.len() as u32)];
        note.extend(directory(camera_settings_at, &camera_settings));
        note.extend(processing_bytes);
        note.extend(&self.preview);
        note
    }
}

/// One directory entry: tag, TIFF type, count, and the value's bytes (little-endian).
struct Entry {
    tag: u16,
    kind: u16,
    count: u32,
    value: Vec<u8>,
}

impl Entry {
    fn long(tag: u16, v: u32) -> Entry {
        Entry { tag, kind: 4, count: 1, value: v.to_le_bytes().to_vec() }
    }
    fn shorts(tag: u16, v: &[u16]) -> Entry {
        Entry { tag, kind: 3, count: v.len() as u32, value: v.iter().flat_map(|x| x.to_le_bytes()).collect() }
    }
    /// A sub-directory pointer (type 13, IFD) to `at` in the note.
    fn offset(tag: u16, at: usize) -> Entry {
        Entry { tag, kind: 13, count: 1, value: (at as u32).to_le_bytes().to_vec() }
    }
}

/// A directory starting at `at` in the note: its entries, no next directory, then the values that don't fit in four bytes.
fn directory(at: usize, entries: &[Entry]) -> Vec<u8> {
    let end = 2 + 12 * entries.len() + 4;
    let mut out = (entries.len() as u16).to_le_bytes().to_vec();
    let mut extra: Vec<u8> = Vec::new();
    for e in entries {
        out.extend(e.tag.to_le_bytes());
        out.extend(e.kind.to_le_bytes());
        out.extend(e.count.to_le_bytes());
        if e.value.len() <= 4 {
            out.extend(&e.value);
            out.extend(std::iter::repeat_n(0u8, 4 - e.value.len()));
        } else {
            out.extend(((at + end + extra.len()) as u32).to_le_bytes());
            extra.extend(&e.value);
        }
    }
    out.extend(0u32.to_le_bytes());
    out.extend(extra);
    out
}
