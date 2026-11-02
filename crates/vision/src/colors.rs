//! What colours a photo is made of, so "red" or "blue sky" can find the photos that are mostly red or blue.
//!
//! No model and no download: [`extract`] looks at a small copy of the photo (display-referred, as rendered for
//! search) and counts its pixels into [`BUCKETS`] (red, orange, yellow, green, teal, blue, purple, pink, brown,
//! black, white, grey), and cuts out the [`SWATCHES`] dominant colours (median cut) with their weights. It is integer
//! arithmetic all the way, so the same pixels give the same palette on any device, and a palette computed on a
//! server, a phone and a desktop merge like the other derived data.
//!
//! One record per photo in an append-only file keyed by content hash ([`ColorIndex`], the same kind of store as the
//! sensitive-content scores): never in the catalog, deleted and made again at will. [`ENGINE`] names the algorithm;
//! a changed one gets a new id, and the old file is started over.

use std::collections::HashMap;
use std::path::Path;

use lightcraft_raster::Rgba8;

use crate::store::{Error, KEY_LEN, Key, RecordFile, Spec};

/// Which algorithm made a palette (the store's model id): bump it when [`extract`] changes.
pub const ENGINE: &str = "lc-colors-1";
/// The colour names a photo's pixels are counted into, in this order.
pub const BUCKETS: [&str; 12] = ["red", "orange", "yellow", "green", "teal", "blue", "purple", "pink", "brown", "black", "white", "grey"];
/// Dominant colours kept per photo.
pub const SWATCHES: usize = 5;
/// Long edge of the copy of the photo that is looked at.
const SAMPLE_EDGE: usize = 64;
/// A bucket holding less than this share of the pixels (out of 255) is left out.
const TINY: u8 = 5;
/// The share (out of 255) a bucket needs for a photo to count as having that colour in a search: about 15 %.
pub const MATCH_MIN: u8 = 38;

/// A dominant colour and how much of the photo it covers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Swatch {
    pub rgb: [u8; 3],
    /// Share of the photo, out of 255.
    pub weight: u8,
}

/// A photo's colours.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Palette {
    /// Share of the photo in each of [`BUCKETS`], out of 255 (a share under 2 % is 0).
    pub buckets: [u8; BUCKETS.len()],
    /// The dominant colours, heaviest first; unused slots have weight 0.
    pub swatches: [Swatch; SWATCHES],
}

impl Palette {
    /// The share (0 to 1) of bucket `bucket` (an index into [`BUCKETS`]).
    pub fn share(&self, bucket: usize) -> f32 {
        f32::from(self.buckets.get(bucket).copied().unwrap_or(0)) / 255.0
    }
}

/// The bucket (index into [`BUCKETS`]) of an sRGB colour.
pub fn bucket_of(rgb: [u8; 3]) -> usize {
    let [r, g, b] = rgb.map(i32::from);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let delta = max - min;
    let light = (max + min) / 2;
    // saturation as HSV has it, out of 255
    let sat = if max == 0 { 0 } else { delta * 255 / max };
    if max < 40 {
        return 9;
    }
    if delta < 14 || sat < 36 {
        return match light {
            200.. => 10,
            ..=50 => 9,
            _ => 11,
        };
    }
    // the hue in degrees, 0..360
    let h = if max == r {
        60 * (g - b) / delta
    } else if max == g {
        120 + 60 * (b - r) / delta
    } else {
        240 + 60 * (r - g) / delta
    }
    .rem_euclid(360);
    match h {
        // red, and its light desaturated kind
        345.. | ..15 if light >= 175 && sat <= 150 => 7,
        345.. | ..15 => 0,
        // dark orange and yellow are brown
        15..50 if max < 175 => 8,
        15..45 => 1,
        45..70 if max < 140 => 8,
        45..70 => 2,
        70..165 => 3,
        165..200 => 4,
        200..255 => 5,
        255..300 => 6,
        _ => 7,
    }
}

/// A copy of `img` at most [`SAMPLE_EDGE`] on a side: every pixel the mean of the block it stands for.
fn sample(img: &Rgba8) -> Vec<[u8; 3]> {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 || img.data.len() < w.saturating_mul(h) {
        return Vec::new();
    }
    let long = w.max(h);
    let (ow, oh) = if long <= SAMPLE_EDGE { (w, h) } else { ((w * SAMPLE_EDGE / long).max(1), (h * SAMPLE_EDGE / long).max(1)) };
    let mut out = Vec::with_capacity(ow * oh);
    for oy in 0..oh {
        let (y0, y1) = (oy * h / oh, ((oy + 1) * h / oh).max(oy * h / oh + 1).min(h));
        for ox in 0..ow {
            let (x0, x1) = (ox * w / ow, ((ox + 1) * w / ow).max(ox * w / ow + 1).min(w));
            let (mut sum, mut n) = ([0u64; 3], 0u64);
            for y in y0..y1 {
                for p in img.data.get(y * w + x0..y * w + x1).unwrap_or(&[]) {
                    for (s, c) in sum.iter_mut().zip(p) {
                        *s += u64::from(*c);
                    }
                    n += 1;
                }
            }
            if let Some(n) = std::num::NonZeroU64::new(n) {
                out.push(sum.map(|s| ((s + n.get() / 2) / n.get()) as u8));
            }
        }
    }
    out
}

/// Median cut: `pixels` into at most [`SWATCHES`] boxes, each as its mean colour and pixel count, heaviest first.
fn cut(pixels: Vec<[u8; 3]>) -> Vec<([u8; 3], usize)> {
    let mut boxes = vec![pixels];
    while boxes.len() < SWATCHES {
        // the box with the most pixels that still has two different colours
        let Some(at) = (0..boxes.len())
            .filter(|&i| boxes.get(i).is_some_and(|b| b.iter().any(|p| Some(p) != b.first())))
            .max_by_key(|&i| (boxes.get(i).map_or(0, Vec::len), std::cmp::Reverse(i)))
        else {
            break;
        };
        let mut b = boxes.swap_remove(at);
        // split across the channel with the widest range, at the median
        let range = |c: usize| {
            let (lo, hi) = b.iter().fold((255u8, 0u8), |(lo, hi), p| (lo.min(p[c]), hi.max(p[c])));
            hi.saturating_sub(lo)
        };
        let channel = (0..3).max_by_key(|&c| (range(c), std::cmp::Reverse(c))).unwrap_or(0);
        // (a total order, so the result doesn't depend on how the sort treats equal pixels)
        b.sort_unstable_by_key(|p| (p.get(channel).copied().unwrap_or(0), *p));
        let upper = b.split_off(b.len() / 2);
        boxes.push(b);
        boxes.push(upper);
    }
    let mut out: Vec<([u8; 3], usize)> = boxes
        .into_iter()
        .filter(|b| !b.is_empty())
        .map(|b| {
            let n = b.len() as u64;
            let mut sum = [0u64; 3];
            for p in &b {
                for (s, c) in sum.iter_mut().zip(p) {
                    *s += u64::from(*c);
                }
            }
            (sum.map(|s| ((s + n / 2) / n) as u8), b.len())
        })
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

/// The colours of `img` (display-referred, any size; alpha is ignored). An empty image has an empty palette.
pub fn extract(img: &Rgba8) -> Palette {
    let pixels = sample(img);
    let total = pixels.len();
    let mut palette = Palette::default();
    if total == 0 {
        return palette;
    }
    let mut counts = [0usize; BUCKETS.len()];
    for p in &pixels {
        if let Some(c) = counts.get_mut(bucket_of(*p)) {
            *c += 1;
        }
    }
    let share = |n: usize| ((n * 255 + total / 2) / total).min(255) as u8;
    for (slot, n) in palette.buckets.iter_mut().zip(counts) {
        let s = share(n);
        *slot = if s < TINY { 0 } else { s };
    }
    for (slot, (rgb, n)) in palette.swatches.iter_mut().zip(cut(pixels)) {
        *slot = Swatch { rgb, weight: share(n) };
    }
    palette
}

/// Bytes of a record: the key, the buckets, then the swatches (red, green, blue, weight).
pub const REC_LEN: usize = KEY_LEN + BUCKETS.len() + SWATCHES * 4;

fn spec(engine: &str) -> Spec<'_> {
    Spec { model: engine, dim: 1, rec_len: REC_LEN }
}

fn encode(key: &Key, p: &Palette) -> Vec<u8> {
    let mut rec = Vec::with_capacity(REC_LEN);
    rec.extend_from_slice(&key.0);
    rec.extend_from_slice(&p.buckets);
    for s in &p.swatches {
        rec.extend_from_slice(&[s.rgb[0], s.rgb[1], s.rgb[2], s.weight]);
    }
    rec
}

fn decode(rec: &[u8]) -> Option<(Key, Palette)> {
    let key = Key::read(rec)?;
    let mut p = Palette::default();
    p.buckets.copy_from_slice(rec.get(KEY_LEN..KEY_LEN + BUCKETS.len())?);
    for (i, s) in p.swatches.iter_mut().enumerate() {
        let at = KEY_LEN + BUCKETS.len() + i * 4;
        let b = rec.get(at..at + 4)?;
        *s = Swatch { rgb: [*b.first()?, *b.get(1)?, *b.get(2)?], weight: *b.get(3)? };
    }
    Some((key, p))
}

/// Every photo's palette.
#[derive(Debug, Default)]
pub struct ColorIndex {
    /// `None`: in memory only.
    file: Option<RecordFile>,
    palettes: HashMap<Key, Palette>,
    revision: u64,
}

impl ColorIndex {
    /// In memory only (tests, a library that isn't a folder).
    pub fn memory() -> ColorIndex {
        ColorIndex::default()
    }

    /// Opens (or creates) the palettes made by algorithm `engine` at `path`. A file of another one is an error.
    pub fn open(path: &Path, engine: &str) -> Result<ColorIndex, Error> {
        let mut palettes = HashMap::new();
        let file = RecordFile::open(path, &spec(engine), |rec| {
            // (a later record of a photo replaces an earlier one)
            if let Some((key, p)) = decode(rec) {
                palettes.insert(key, p);
            }
        })?;
        Ok(ColorIndex { file: Some(file), palettes, revision: 0 })
    }

    pub fn len(&self) -> usize {
        self.palettes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.palettes.is_empty()
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn contains(&self, key: &Key) -> bool {
        self.palettes.contains_key(key)
    }
    pub fn get(&self, key: &Key) -> Option<&Palette> {
        self.palettes.get(key)
    }
    /// The photos with a palette.
    pub fn keys(&self) -> impl Iterator<Item = Key> + '_ {
        self.palettes.keys().copied()
    }

    /// Keep `palette` for `key` (written to the file first: a failed write keeps nothing).
    pub fn insert(&mut self, key: Key, palette: Palette) -> Result<(), Error> {
        if let Some(file) = self.file.as_mut() {
            file.append(&encode(&key, &palette))?;
        }
        self.palettes.insert(key, palette);
        self.revision += 1;
        Ok(())
    }

    /// The photos that have every colour of `buckets` (indices into [`BUCKETS`]) at [`MATCH_MIN`] or more, most of it
    /// first: with the sum of their shares (0 to the number of colours) as the score.
    pub fn search(&self, buckets: &[usize]) -> Vec<(Key, f32)> {
        if buckets.is_empty() {
            return Vec::new();
        }
        let mut out: Vec<(Key, f32)> = self
            .palettes
            .iter()
            .filter(|(_, p)| buckets.iter().all(|&b| p.buckets.get(b).is_some_and(|s| *s >= MATCH_MIN)))
            .map(|(k, p)| (*k, buckets.iter().map(|&b| p.share(b)).sum()))
            .collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.0.cmp(&b.0.0)));
        out
    }

    /// The score of `key` (as [`ColorIndex::search`] gives it) when it has every colour of `buckets`.
    pub fn matches(&self, key: &Key, buckets: &[usize]) -> Option<f32> {
        let p = self.palettes.get(key)?;
        buckets.iter().all(|&b| p.buckets.get(b).is_some_and(|s| *s >= MATCH_MIN)).then(|| buckets.iter().map(|&b| p.share(b)).sum())
    }

    /// Forget every palette.
    pub fn clear(&mut self) -> Result<(), Error> {
        if let Some(file) = self.file.as_mut() {
            file.truncate()?;
        }
        self.palettes.clear();
        self.revision += 1;
        Ok(())
    }
}

/// The colour names in a query, in English and a few languages of the app: `(word, bucket)`, lower case.
// ponytail: whole words only, no CJK (no spaces to split on); add a segmenter if Chinese or Japanese users ask.
const WORDS: &[(&str, usize)] = &[
    ("red", 0),
    ("reddish", 0),
    ("rot", 0),
    ("rojo", 0),
    ("rouge", 0),
    ("rosso", 0),
    ("vermelho", 0),
    ("orange", 1),
    ("orangish", 1),
    ("naranja", 1),
    ("arancione", 1),
    ("laranja", 1),
    ("yellow", 2),
    ("yellowish", 2),
    ("gelb", 2),
    ("amarillo", 2),
    ("jaune", 2),
    ("giallo", 2),
    ("amarelo", 2),
    ("green", 3),
    ("greenish", 3),
    ("grün", 3),
    ("gruen", 3),
    ("verde", 3),
    ("vert", 3),
    ("teal", 4),
    ("cyan", 4),
    ("turquoise", 4),
    ("aqua", 4),
    ("türkis", 4),
    ("turquesa", 4),
    ("turchese", 4),
    ("blue", 5),
    ("bluish", 5),
    ("blau", 5),
    ("azul", 5),
    ("bleu", 5),
    ("blu", 5),
    ("purple", 6),
    ("violet", 6),
    ("lila", 6),
    ("morado", 6),
    ("violeta", 6),
    ("viola", 6),
    ("roxo", 6),
    ("pink", 7),
    ("pinkish", 7),
    ("magenta", 7),
    ("rosa", 7),
    ("brown", 8),
    ("brownish", 8),
    ("braun", 8),
    ("marrón", 8),
    ("marron", 8),
    ("marrone", 8),
    ("marrom", 8),
    ("black", 9),
    ("schwarz", 9),
    ("negro", 9),
    ("noir", 9),
    ("nero", 9),
    ("preto", 9),
    ("white", 10),
    ("weiß", 10),
    ("weiss", 10),
    ("blanco", 10),
    ("blanc", 10),
    ("bianco", 10),
    ("branco", 10),
    ("grey", 11),
    ("gray", 11),
    ("grau", 11),
    ("gris", 11),
    ("grigio", 11),
    ("cinza", 11),
];

/// A query split into the colours it names and the rest of it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ColorQuery {
    /// Indices into [`BUCKETS`], each once, in the order named.
    pub buckets: Vec<usize>,
    /// What is left of the query once the colour words (and an "and" between them) are taken out.
    pub rest: String,
}

/// Which colours `query` names ("red", "blue sky", "black and white"), and what is left of it ("", "sky"). A query
/// that names none has an empty `buckets`; its `rest` is then the whole query.
pub fn parse(query: &str) -> ColorQuery {
    let mut out = ColorQuery::default();
    let mut rest: Vec<&str> = Vec::new();
    let mut after_color = false;
    for word in query.split(|c: char| !c.is_alphabetic()).filter(|w| !w.is_empty()) {
        let lower = word.to_lowercase();
        match WORDS.iter().find(|(w, _)| *w == lower) {
            Some((_, b)) => {
                if !out.buckets.contains(b) {
                    out.buckets.push(*b);
                }
                after_color = true;
            }
            // ("black and white")
            None if after_color && matches!(lower.as_str(), "and" | "und" | "y" | "et" | "e") => {}
            None => {
                rest.push(word);
                after_color = false;
            }
        }
    }
    if !out.buckets.is_empty() {
        out.rest = rest.join(" ");
    } else {
        out.rest = query.trim().to_string();
    }
    out
}

/// Every word [`parse`] takes for a colour, with its bucket.
pub fn words() -> Vec<(&'static str, usize)> {
    WORDS.to_vec()
}

/// The name of bucket `i`.
pub fn name(i: usize) -> &'static str {
    BUCKETS.get(i).copied().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, rgb: [u8; 3]) -> Rgba8 {
        Rgba8::from_fn(w, h, |_, _| [rgb[0], rgb[1], rgb[2], 255])
    }

    fn red() -> usize {
        0
    }

    fn named(n: &str) -> usize {
        BUCKETS.iter().position(|b| *b == n).unwrap()
    }

    fn key(n: u8) -> Key {
        Key([n; KEY_LEN])
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lc-colors-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("colors.bin")
    }

    #[test]
    fn a_red_square_is_all_red() {
        let p = extract(&solid(300, 200, [220, 20, 30]));
        assert_eq!(p.buckets[red()], 255);
        assert_eq!(p.buckets.iter().filter(|b| **b > 0).count(), 1);
        assert_eq!(p.swatches[0], Swatch { rgb: [220, 20, 30], weight: 255 });
        assert_eq!(p.swatches[1].weight, 0, "one colour, one swatch");
    }

    #[test]
    fn half_green_half_blue_is_both() {
        let img = Rgba8::from_fn(100, 80, |x, _| if x < 50 { [20, 200, 40, 255] } else { [30, 60, 220, 255] });
        let p = extract(&img);
        let (g, b) = (p.buckets[named("green")], p.buckets[named("blue")]);
        assert!((120..=135).contains(&g) && (120..=135).contains(&b), "{g} {b}");
        let colours: Vec<[u8; 3]> = p.swatches.iter().filter(|s| s.weight > 0).map(|s| s.rgb).collect();
        assert!(colours.contains(&[20, 200, 40]) && colours.contains(&[30, 60, 220]), "{colours:?}");
        assert_eq!(p.buckets[red()], 0);
    }

    #[test]
    fn the_same_pixels_give_the_same_palette_whatever_the_size_or_order_they_come_in() {
        let img = Rgba8::from_fn(517, 331, |x, y| [(x * 7 % 256) as u8, (y * 5 % 256) as u8, ((x + y) % 256) as u8, 255]);
        let a = extract(&img);
        assert_eq!(a, extract(&img.clone()));
        // alpha doesn't matter
        let mut other = img.clone();
        other.data.iter_mut().for_each(|p| p[3] = 17);
        assert_eq!(a, extract(&other));
        assert!(a.swatches.windows(2).all(|w| w[0].weight >= w[1].weight), "heaviest first");
    }

    #[test]
    fn the_names_of_colours_match_what_people_call_them() {
        for (rgb, want) in [
            ([230, 30, 30], "red"),
            ([240, 130, 20], "orange"),
            ([250, 220, 30], "yellow"),
            ([40, 170, 60], "green"),
            ([20, 170, 170], "teal"),
            ([30, 80, 220], "blue"),
            ([130, 50, 190], "purple"),
            ([240, 110, 180], "pink"),
            ([255, 190, 200], "pink"),
            ([120, 75, 40], "brown"),
            ([5, 5, 8], "black"),
            ([30, 30, 30], "black"),
            ([250, 250, 248], "white"),
            ([128, 128, 128], "grey"),
        ] {
            assert_eq!(name(bucket_of(rgb)), want, "{rgb:?}");
        }
    }

    #[test]
    fn a_speck_of_a_colour_is_left_out() {
        let img = Rgba8::from_fn(64, 64, |x, y| if x == 0 && y == 0 { [255, 0, 0, 255] } else { [128, 128, 128, 255] });
        let p = extract(&img);
        assert_eq!(p.buckets[red()], 0);
        assert!(p.buckets[named("grey")] >= 250);
    }

    #[test]
    fn nothing_to_look_at_is_an_empty_palette_not_a_panic() {
        assert_eq!(extract(&Rgba8 { width: 0, height: 0, data: vec![] }), Palette::default());
        // a width and height that claim more pixels than there are
        assert_eq!(extract(&Rgba8 { width: usize::MAX, height: 2, data: vec![[1, 2, 3, 255]; 4] }), Palette::default());
        assert_eq!(extract(&Rgba8 { width: 10, height: 10, data: vec![[1, 2, 3, 255]; 4] }), Palette::default());
        assert_eq!(extract(&solid(1, 1, [255, 0, 0])).buckets[red()], 255);
    }

    #[test]
    fn the_palettes_are_kept_in_the_file_and_the_later_one_counts() {
        let path = temp("file");
        let mut ix = ColorIndex::open(&path, ENGINE).unwrap();
        ix.insert(key(1), extract(&solid(8, 8, [220, 20, 30]))).unwrap();
        ix.insert(key(2), extract(&solid(8, 8, [30, 60, 220]))).unwrap();
        ix.insert(key(1), extract(&solid(8, 8, [30, 200, 40]))).unwrap();
        drop(ix);
        let ix = ColorIndex::open(&path, ENGINE).unwrap();
        assert_eq!(ix.len(), 2);
        assert_eq!(ix.get(&key(1)).unwrap().buckets[named("green")], 255, "the later record replaces the earlier");
        assert_eq!(ix.search(&[named("blue")]).iter().map(|(k, _)| k.0[0]).collect::<Vec<_>>(), [2]);
        assert!(ix.search(&[named("red")]).is_empty());
        assert!(ix.search(&[]).is_empty());
        assert!(ix.matches(&key(2), &[named("blue")]).is_some() && ix.matches(&key(2), &[named("blue"), named("red")]).is_none());
        // another algorithm's file is refused, not mixed in
        assert!(matches!(ColorIndex::open(&path, "lc-colors-2"), Err(Error::Mismatch { .. })));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_cut_short_or_foreign_file_is_cut_or_refused_not_a_panic() {
        let path = temp("damaged");
        let mut ix = ColorIndex::open(&path, ENGINE).unwrap();
        ix.insert(key(1), extract(&solid(8, 8, [220, 20, 30]))).unwrap();
        drop(ix);
        // a record cut in the middle (a crash while writing)
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.extend_from_slice(&[7u8; REC_LEN - 9]);
        std::fs::write(&path, &bytes).unwrap();
        let ix = ColorIndex::open(&path, ENGINE).unwrap();
        assert_eq!(ix.len(), 1, "the whole record stays, the partial one goes");
        drop(ix);
        // a header cut short, garbage, an empty file
        for junk in [&bytes[..10], &[0xffu8; 100][..], &[][..]] {
            std::fs::write(&path, junk).unwrap();
            let _ = ColorIndex::open(&path, ENGINE);
        }
        // a whole record of nonsense is a palette of nonsense, not a crash
        let mut ix = ColorIndex::open(&path, ENGINE).unwrap();
        ix.insert(key(3), Palette { buckets: [255; 12], swatches: [Swatch { rgb: [255; 3], weight: 255 }; SWATCHES] }).unwrap();
        assert_eq!(ix.search(&[0, 1, 2]).len(), 1);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn colour_words_are_found_in_a_query() {
        let q = parse("red");
        assert_eq!((q.buckets.as_slice(), q.rest.as_str()), (&[0usize][..], ""));
        let q = parse("Blue sky");
        assert_eq!((q.buckets.as_slice(), q.rest.as_str()), (&[5usize][..], "sky"));
        let q = parse("black and white");
        assert_eq!((q.buckets.as_slice(), q.rest.as_str()), (&[9usize, 10][..], ""));
        let q = parse("a reddish car, red again");
        assert_eq!((q.buckets.as_slice(), q.rest.as_str()), (&[0usize][..], "a car again"));
        let q = parse("grün");
        assert_eq!(q.buckets, [3]);
        let q = parse("a dog on a beach");
        assert!(q.buckets.is_empty());
        assert_eq!(q.rest, "a dog on a beach");
        // (words that merely contain a colour name are not colours)
        assert!(parse("bored shredded").buckets.is_empty());
        assert!(parse("").buckets.is_empty());
    }
}
