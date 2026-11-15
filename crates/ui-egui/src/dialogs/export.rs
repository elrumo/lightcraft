//! The pieces of the Export dialog that are shared: the fork's Export dialog itself is `panels/export.rs`.

use super::*;

/// Two columns of settings-group checkboxes (Copy Settings, Create Preset) plus All / None.
pub(super) fn group_checklist(ui: &mut egui::Ui, tag: &str, groups: &mut Vec<String>) {
    let key_of = |g: &SettingsGroup| serde_json::to_value(g).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    ui.columns(2, |cols| {
        for (i, g) in SettingsGroup::ALL.iter().enumerate() {
            let key = key_of(g);
            let mut on = groups.contains(&key);
            let r = cols[i % 2].checkbox(&mut on, crate::i18n::tr(g.label()));
            crate::widgets::register_response(format!("{tag}:{key}"), r.rect, &r);
            if r.changed() {
                if on {
                    groups.push(key);
                } else {
                    groups.retain(|x| *x != key);
                }
            }
        }
    });
    ui.horizontal(|ui| {
        let all = ui.small_button(crate::i18n::tr("All"));
        crate::widgets::register(ui, format!("{tag}:all"), all.rect);
        if all.clicked() {
            *groups = SettingsGroup::ALL.iter().map(key_of).collect();
        }
        let none = ui.small_button(crate::i18n::tr("None"));
        crate::widgets::register(ui, format!("{tag}:none"), none.rect);
        if none.clicked() {
            groups.clear();
        }
    });
}

/// Width of the label column in dialogs.
pub(super) const LABEL_W: f32 = 78.0;

/// The 16-bit note: LZW's byte dictionary does worse on 16-bit samples than storing them, so
/// 16-bit LZW TIFFs came out 10-33 % larger than uncompressed ones (issue #2882).
pub(crate) const TIFF_LZW_NOTE_16: &str =
    "At 16 bit, LZW is often larger than no compression, and larger and slower than ZIP. Use it only if other software needs LZW.";
/// The note at 8 bit and 32-bit float: one 8-bit file in #2882 was larger with LZW too.
pub(crate) const TIFF_LZW_NOTE: &str =
    "LZW is usually larger and slower than ZIP, and can be larger than no compression. Use it only if other software needs LZW.";

/// The note shown under Compression when a TIFF export uses LZW (none otherwise). The export
/// keeps LZW: some software reads only LZW TIFFs. (The Export dialog, `panels/export.rs`, shows it.)
pub(crate) fn tiff_lzw_note(o: &lightcraft_engine::export::ExportOptions) -> Option<&'static str> {
    use lightcraft_engine::export::{ExportFormat, OutputDepth, TiffCompression};
    if o.format != ExportFormat::Tiff || o.tiff_compression != TiffCompression::Lzw {
        return None;
    }
    Some(if o.effective_depth() == OutputDepth::U16 { TIFF_LZW_NOTE_16 } else { TIFF_LZW_NOTE })
}

/// [`tiff_lzw_note`] for an export preset's params: the hint on presets that write LZW TIFFs.
pub(crate) fn preset_lzw_note(params: &serde_json::Value) -> Option<&'static str> {
    tiff_lzw_note(&lightcraft_engine::export::ExportOptions::from_json(params))
}

#[cfg(test)]
mod tiff_lzw_note_tests {
    use lightcraft_engine::export::{ExportFormat, ExportOptions, TiffCompression};
    use serde_json::json;

    use super::{TIFF_LZW_NOTE, TIFF_LZW_NOTE_16, preset_lzw_note, tiff_lzw_note};

    fn tiff(compression: TiffCompression, bit_depth: Option<u8>) -> ExportOptions {
        ExportOptions { format: ExportFormat::Tiff, tiff_compression: compression, bit_depth, ..Default::default() }
    }

    // Issue #2882: the note shows for LZW only, worded more strongly at 16 bit (TIFF's default depth)
    #[test]
    fn lzw_gets_a_note_stronger_at_16_bit() {
        assert_eq!(tiff_lzw_note(&tiff(TiffCompression::Lzw, None)), Some(TIFF_LZW_NOTE_16), "TIFF defaults to 16-bit");
        assert_eq!(tiff_lzw_note(&tiff(TiffCompression::Lzw, Some(16))), Some(TIFF_LZW_NOTE_16));
        assert_eq!(tiff_lzw_note(&tiff(TiffCompression::Lzw, Some(8))), Some(TIFF_LZW_NOTE));
        assert_eq!(tiff_lzw_note(&tiff(TiffCompression::Lzw, Some(32))), Some(TIFF_LZW_NOTE));
        for compression in [TiffCompression::None, TiffCompression::Deflate, TiffCompression::PackBits] {
            for depth in [None, Some(8), Some(16), Some(32)] {
                assert_eq!(tiff_lzw_note(&tiff(compression, depth)), None, "{compression:?} {depth:?}");
            }
        }
        // the setting only matters for TIFF: a JPEG preset that carries it gets no note
        let jpeg = ExportOptions { format: ExportFormat::Jpeg, tiff_compression: TiffCompression::Lzw, ..Default::default() };
        assert_eq!(tiff_lzw_note(&jpeg), None);
        assert!(TIFF_LZW_NOTE_16.contains("16 bit") && TIFF_LZW_NOTE.contains("other software needs LZW"));
    }

    // Presets that write LZW TIFFs carry the same hint; the built-in ones don't write LZW
    #[test]
    fn presets_with_lzw_get_the_same_hint() {
        assert_eq!(preset_lzw_note(&json!({"format": "tiff", "tiffCompression": "lzw"})), Some(TIFF_LZW_NOTE_16));
        assert_eq!(preset_lzw_note(&json!({"format": "tiff", "tiffCompression": "LZW", "bitDepth": 8})), Some(TIFF_LZW_NOTE));
        assert_eq!(preset_lzw_note(&json!({"format": "tiff", "tiffCompression": "zip"})), None);
        assert_eq!(preset_lzw_note(&json!({"format": "tiff"})), None, "ZIP is the default");
        for p in lightcraft_engine::export::builtin_presets() {
            assert_eq!(preset_lzw_note(&p.params), None, "{}", p.name);
        }
    }
}
