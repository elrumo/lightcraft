//! Design tokens (colours, sizes, fonts). Values measured from black-box observation of the
//! reference app's dark theme (see plan/lightroom/10-observed-ui.md); everything is our own code.
//!
//! Themes come in two families, picked by the Appearance Mode setting (Settings ▸ Interface):
//! - dark: **Charcoal** (the default, the observed dark look), **Midnight** (near black) and
//!   **Studio Dark**;
//! - light: **Silver** (light grey chrome), **Paper** (near white) and **Studio Light**.
//!
//! The two Studio themes are one design in both appearances, the look of the server's admin page
//! (`apps/lightcraft-server/admin`): its neutral greys and blue accent, panels floating as rounded
//! cards on the window's backdrop, borderless capsule buttons, rounder fields, menus and dialogs,
//! and sliders with a filled track and a solid handle. Shape is part of the tokens
//! ([`Tokens::card_radius`] and the radii after it), so the classic themes keep their flush, square look.
//!
//! The compact (phone / tablet) layout has its own look instead, iOS's light or dark one
//! ([`Tokens::ios_for`], [`apply_compact`]).

use std::sync::Arc;

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, Visuals};

pub const FONT_SEMIBOLD: &str = "semibold";
const WEB_CHINESE_FONT: &str = "web Noto Sans CJK SC";

/// Shared colour-label palette for badges, thumbnail surrounds and feedback.
pub fn label_color(label: lightcraft_catalog::ColorLabel) -> Color32 {
    use lightcraft_catalog::ColorLabel;
    match label {
        ColorLabel::Red => Color32::from_rgb(222, 72, 72),
        ColorLabel::Yellow => Color32::from_rgb(232, 196, 58),
        ColorLabel::Green => Color32::from_rgb(88, 176, 92),
        ColorLabel::Blue => Color32::from_rgb(72, 130, 222),
        ColorLabel::Purple => Color32::from_rgb(158, 100, 210),
    }
}

/// A translucent label colour over thumbnail chrome; selection remains brighter.
pub fn label_background(base: Color32, label: Option<lightcraft_catalog::ColorLabel>, selected: bool) -> Color32 {
    label.map_or(base, |l| base.lerp_to_gamma(label_color(l), if selected { 0.32 } else { 0.22 }))
}

/// Label confirmations use a pale colour and dark text, distinct from neutral/error HUDs.
pub fn label_toast_colors(label: lightcraft_catalog::ColorLabel) -> (Color32, Color32) {
    (Color32::WHITE.lerp_to_gamma(label_color(label), 0.3), Color32::from_rgb(36, 24, 24))
}

#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    /// Top bar, side panels, bottom bar, tool strip.
    pub chrome: Color32,
    /// Photo canvas (detail view) and filmstrip.
    pub canvas: Color32,
    /// Grid background behind the cells.
    pub grid_bg: Color32,
    pub cell: Color32,
    pub cell_selected: Color32,
    pub divider: Color32,
    pub inset: Color32,
    pub field: Color32,
    pub field_border: Color32,
    pub button: Color32,
    pub button_border: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub tool_active: Color32,
    pub text: Color32,
    pub text_label: Color32,
    pub text_dim: Color32,
    pub text_disabled: Color32,
    /// Icon glyphs in the chrome.
    pub icon: Color32,
    pub track: Color32,
    pub thumb: Color32,
    pub thumb_hover: Color32,
    pub accent: Color32,
    pub star: Color32,
    pub pick: Color32,
    pub reject: Color32,
    /// Ratings and flags drawn over a photo (a thumbnail's badges): light in both appearances, as
    /// the photo under them isn't the page's colour.
    pub photo_star: Color32,
    pub photo_pick: Color32,
    /// Cautionary notices (e.g. a raw shown from its embedded preview): a muted amber.
    pub caution: Color32,
    pub mask_overlay: Color32,
    /// An on / off switch's track when off, and when on (iOS's green).
    pub switch_off: Color32,
    pub switch_on: Color32,
    /// The chosen segment of a segmented control (raised on its track).
    pub segment_pill: Color32,
    /// How dark the scrim behind a page or an action sheet gets (alpha of black, 0–255).
    pub scrim: u8,
    /// What the panels sit on: the top bar and the tool strip (`chrome` in the classic themes).
    pub backdrop: Color32,
    // shape
    /// Docked panels' corner radius. 0 (the classic themes): panels meet edge to edge, `dock_gap`
    /// apart. More: each is a card with rounded corners floating on `backdrop`, `dock_gap` from its
    /// neighbours and from the window's edges, and the tool strip is a card of its own.
    pub card_radius: f32,
    /// The gap between docked panels (where their resize handles are).
    pub dock_gap: f32,
    /// Corner radius of egui's controls (buttons, fields, combo boxes) and the tool strip's squares.
    pub control_radius: u8,
    /// Corner radius of our own push buttons (Auto, B&W…) and segmented controls; 255 for capsules.
    pub button_radius: u8,
    /// Corner radius of windows, dialogs, menus and popups.
    pub window_radius: u8,
    /// Desktop sliders drawn the Studio way: a thicker rounded track filled with `thumb` from the
    /// control's default to the handle, and a solid handle (rather than a hollow ring on a hairline).
    pub filled_sliders: bool,
    // metrics (points)
    pub top_bar_h: f32,
    pub bottom_bar_h: f32,
    pub panel_w: f32,
    pub strip_w: f32,
    pub slider_row_h: f32,
    pub section_h: f32,
    pub film_h: f32,
}

/// An interface theme. Each belongs to the dark or the light family.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ThemeKind {
    #[default]
    Charcoal,
    Midnight,
    Silver,
    Paper,
    /// The Studio design in the dark appearance (the server admin page's dark look).
    StudioDark,
    /// The Studio design in the light appearance (the server admin page's light look).
    StudioLight,
}

impl ThemeKind {
    pub const ALL: [ThemeKind; 6] =
        [ThemeKind::Charcoal, ThemeKind::Midnight, ThemeKind::StudioDark, ThemeKind::Silver, ThemeKind::Paper, ThemeKind::StudioLight];

    pub fn is_dark(self) -> bool {
        matches!(self, ThemeKind::Charcoal | ThemeKind::Midnight | ThemeKind::StudioDark)
    }

    /// The Studio design (floating panel cards, capsule buttons), in either appearance.
    pub fn is_studio(self) -> bool {
        matches!(self, ThemeKind::StudioDark | ThemeKind::StudioLight)
    }

    /// The theme's id (settings value and command suffix).
    pub fn id(self) -> &'static str {
        match self {
            ThemeKind::Charcoal => "charcoal",
            ThemeKind::Midnight => "midnight",
            ThemeKind::Silver => "silver",
            ThemeKind::Paper => "paper",
            ThemeKind::StudioDark => "studioDark",
            ThemeKind::StudioLight => "studioLight",
        }
    }

    /// The English display name.
    pub fn label(self) -> &'static str {
        match self {
            ThemeKind::Charcoal => "Charcoal",
            ThemeKind::Midnight => "Midnight",
            ThemeKind::Silver => "Silver",
            ThemeKind::Paper => "Paper",
            ThemeKind::StudioDark => "Studio Dark",
            ThemeKind::StudioLight => "Studio Light",
        }
    }
}

impl Default for Tokens {
    fn default() -> Self {
        Tokens {
            chrome: Color32::from_rgb(0x2d, 0x2d, 0x2d),
            canvas: Color32::from_rgb(0x1c, 0x1c, 0x1c),
            grid_bg: Color32::from_rgb(0x0f, 0x0f, 0x0f),
            cell: Color32::from_rgb(0x1c, 0x1c, 0x1c),
            cell_selected: Color32::from_rgb(0x2d, 0x2d, 0x2d),
            divider: Color32::from_rgb(0x1c, 0x1c, 0x1c),
            inset: Color32::from_rgb(0x23, 0x23, 0x23),
            field: Color32::from_rgb(0x22, 0x22, 0x22),
            field_border: Color32::from_rgb(0x3c, 0x3c, 0x3c),
            button: Color32::from_rgb(0x24, 0x24, 0x24),
            button_border: Color32::from_rgb(0x4c, 0x4c, 0x4c),
            hover: Color32::from_rgb(0x3a, 0x3a, 0x3a),
            pressed: Color32::from_rgb(0x3f, 0x3f, 0x3f),
            tool_active: Color32::from_rgb(0x3f, 0x3f, 0x3f),
            text: Color32::from_rgb(0xe2, 0xe2, 0xe2),
            text_label: Color32::from_rgb(0xbc, 0xbc, 0xbc),
            text_dim: Color32::from_rgb(0x8e, 0x8e, 0x8e),
            text_disabled: Color32::from_rgb(0x5c, 0x5c, 0x5c),
            icon: Color32::from_rgb(0x9a, 0x9a, 0x9a),
            track: Color32::from_rgb(0x5a, 0x5a, 0x5a),
            thumb: Color32::from_rgb(0xa0, 0xa0, 0xa0),
            thumb_hover: Color32::from_rgb(0xe0, 0xe0, 0xe0),
            accent: Color32::from_rgb(0x01, 0x65, 0xdd),
            star: Color32::from_rgb(0xd8, 0xd8, 0xd8),
            pick: Color32::from_rgb(0xf0, 0xf0, 0xf0),
            photo_star: Color32::from_rgb(0xd8, 0xd8, 0xd8),
            photo_pick: Color32::from_rgb(0xf0, 0xf0, 0xf0),
            reject: Color32::from_rgb(0xe0, 0x4a, 0x4a),
            caution: Color32::from_rgb(0xe3, 0xa8, 0x3c),
            mask_overlay: Color32::from_rgba_unmultiplied(0xe0, 0x20, 0x30, 110),
            switch_off: Color32::from_rgb(0x39, 0x39, 0x3d),
            switch_on: Color32::from_rgb(0x30, 0xd1, 0x58),
            segment_pill: Color32::from_rgb(0x63, 0x63, 0x66),
            scrim: 150,
            backdrop: Color32::from_rgb(0x2d, 0x2d, 0x2d),
            card_radius: 0.0,
            dock_gap: 4.0,
            control_radius: 4,
            button_radius: 4,
            window_radius: 6,
            filled_sliders: false,
            top_bar_h: 42.0,
            bottom_bar_h: 48.0,
            panel_w: 270.0,
            strip_w: 48.0,
            slider_row_h: 45.0,
            section_h: 57.0,
            film_h: 142.0,
        }
    }
}

impl Tokens {
    /// The phone / tablet (compact) look, as close to iOS's own dark appearance as egui allows:
    /// Apple's published dark-mode system colours (system blue, grouped backgrounds, separators,
    /// label greys), black behind photos as in Lightroom's mobile app.
    pub fn ios() -> Tokens {
        Tokens::ios_for(true)
    }

    /// The phone / tablet look in iOS's light or dark appearance.
    pub fn ios_for(dark: bool) -> Tokens {
        if dark { Tokens::ios_dark() } else { Tokens::ios_light() }
    }

    /// iOS's light appearance: its system colours (`systemBackground` white behind photos and bars,
    /// `secondarySystemBackground` for sheets and pages, system blue), the same roles as the dark ones.
    fn ios_light() -> Tokens {
        let rgb = Color32::from_rgb;
        Tokens {
            chrome: rgb(0xf2, 0xf2, 0xf7),
            backdrop: rgb(0xf2, 0xf2, 0xf7),
            canvas: rgb(0xff, 0xff, 0xff),
            grid_bg: rgb(0xff, 0xff, 0xff),
            cell: rgb(0xf2, 0xf2, 0xf7),
            cell_selected: rgb(0xff, 0xff, 0xff),
            divider: rgb(0xc6, 0xc6, 0xc8),
            inset: rgb(0xe5, 0xe5, 0xea),
            field: rgb(0xe5, 0xe5, 0xea),
            field_border: rgb(0xe5, 0xe5, 0xea),
            button: rgb(0xe5, 0xe5, 0xea),
            button_border: rgb(0xe5, 0xe5, 0xea),
            hover: rgb(0xd1, 0xd1, 0xd6),
            pressed: rgb(0xc7, 0xc7, 0xcc),
            tool_active: rgb(0xe5, 0xe5, 0xea),
            text: rgb(0x00, 0x00, 0x00),
            text_label: rgb(0x1c, 0x1c, 0x1e),
            text_dim: rgb(0x8e, 0x8e, 0x93),
            text_disabled: rgb(0xc7, 0xc7, 0xcc),
            icon: rgb(0x3c, 0x3c, 0x43),
            track: rgb(0xd1, 0xd1, 0xd6),
            thumb: rgb(0xff, 0xff, 0xff),
            thumb_hover: rgb(0xff, 0xff, 0xff),
            accent: rgb(0x00, 0x7a, 0xff),
            star: rgb(0x3c, 0x3c, 0x43),
            pick: rgb(0x1c, 0x1c, 0x1e),
            photo_star: rgb(0xe5, 0xe5, 0xea),
            photo_pick: rgb(0xff, 0xff, 0xff),
            reject: rgb(0xff, 0x3b, 0x30),
            caution: rgb(0xff, 0x95, 0x00),
            switch_off: rgb(0xe5, 0xe5, 0xea),
            switch_on: rgb(0x34, 0xc7, 0x59),
            segment_pill: rgb(0xff, 0xff, 0xff),
            scrim: 90,
            ..Tokens::default()
        }
    }

    /// iOS's dark appearance: Apple's dark-mode system colours.
    fn ios_dark() -> Tokens {
        let rgb = Color32::from_rgb;
        Tokens {
            chrome: rgb(0x1c, 0x1c, 0x1e),
            backdrop: rgb(0x1c, 0x1c, 0x1e),
            canvas: rgb(0x00, 0x00, 0x00),
            grid_bg: rgb(0x00, 0x00, 0x00),
            cell: rgb(0x1c, 0x1c, 0x1e),
            cell_selected: rgb(0x2c, 0x2c, 0x2e),
            divider: rgb(0x38, 0x38, 0x3a),
            inset: rgb(0x2c, 0x2c, 0x2e),
            field: rgb(0x2c, 0x2c, 0x2e),
            field_border: rgb(0x2c, 0x2c, 0x2e),
            button: rgb(0x2c, 0x2c, 0x2e),
            button_border: rgb(0x2c, 0x2c, 0x2e),
            hover: rgb(0x3a, 0x3a, 0x3c),
            pressed: rgb(0x48, 0x48, 0x4a),
            tool_active: rgb(0x3a, 0x3a, 0x3c),
            text: rgb(0xff, 0xff, 0xff),
            text_label: rgb(0xe5, 0xe5, 0xea),
            text_dim: rgb(0x8e, 0x8e, 0x93),
            text_disabled: rgb(0x48, 0x48, 0x4a),
            icon: rgb(0xe5, 0xe5, 0xea),
            track: rgb(0x48, 0x48, 0x4a),
            thumb: rgb(0xe5, 0xe5, 0xea),
            thumb_hover: rgb(0xff, 0xff, 0xff),
            accent: rgb(0x0a, 0x84, 0xff),
            star: rgb(0xe5, 0xe5, 0xea),
            pick: rgb(0xff, 0xff, 0xff),
            photo_star: rgb(0xe5, 0xe5, 0xea),
            photo_pick: rgb(0xff, 0xff, 0xff),
            reject: rgb(0xff, 0x45, 0x3a),
            caution: rgb(0xff, 0xd6, 0x0a),
            ..Tokens::default()
        }
    }

    /// Whether the UI is drawn dark now (a light desktop theme or the compact layout's light look: not).
    pub fn is_dark(ctx: &egui::Context) -> bool {
        ctx.data(|d| d.get_temp::<bool>(egui::Id::new("lc-dark"))).unwrap_or(true)
    }

    /// The tokens of `kind`. Metrics are shared; the classic themes change only colours, the Studio
    /// ones shape too.
    pub fn for_kind(kind: ThemeKind) -> Tokens {
        let rgb = |v: u32| Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
        let base = Tokens::default();
        let tokens = match kind {
            ThemeKind::Charcoal => base,
            ThemeKind::Midnight => Tokens {
                chrome: rgb(0x1e1e1e),
                canvas: rgb(0x121212),
                grid_bg: rgb(0x080808),
                cell: rgb(0x121212),
                cell_selected: rgb(0x262626),
                divider: rgb(0x0e0e0e),
                inset: rgb(0x181818),
                field: rgb(0x161616),
                field_border: rgb(0x333333),
                button: rgb(0x191919),
                button_border: rgb(0x404040),
                hover: rgb(0x2e2e2e),
                pressed: rgb(0x343434),
                tool_active: rgb(0x343434),
                ..base
            },
            ThemeKind::Silver => Tokens {
                chrome: rgb(0xe6e6e6),
                canvas: rgb(0xcfcfcf),
                grid_bg: rgb(0xd9d9d9),
                cell: rgb(0xcbcbcb),
                cell_selected: rgb(0xb4b4b4),
                divider: rgb(0xc4c4c4),
                inset: rgb(0xdcdcdc),
                field: rgb(0xf6f6f6),
                field_border: rgb(0xb8b8b8),
                button: rgb(0xf2f2f2),
                button_border: rgb(0xb0b0b0),
                hover: rgb(0xd2d2d2),
                pressed: rgb(0xc4c4c4),
                tool_active: rgb(0xcacaca),
                text: rgb(0x1c1c1c),
                text_label: rgb(0x383838),
                text_dim: rgb(0x6a6a6a),
                text_disabled: rgb(0xa2a2a2),
                icon: rgb(0x555555),
                track: rgb(0xa8a8a8),
                thumb: rgb(0x5e5e5e),
                thumb_hover: rgb(0x262626),
                accent: rgb(0x0a62d0),
                star: rgb(0x3a3a3a),
                pick: rgb(0x2a2a2a),
                reject: rgb(0xc83434),
                caution: rgb(0xb0761a),
                ..base
            },
            ThemeKind::Paper => Tokens {
                chrome: rgb(0xfafafa),
                canvas: rgb(0xececec),
                grid_bg: rgb(0xf2f2f2),
                cell: rgb(0xe8e8e8),
                cell_selected: rgb(0xd2d2d2),
                divider: rgb(0xe0e0e0),
                inset: rgb(0xf0f0f0),
                field: rgb(0xffffff),
                field_border: rgb(0xcccccc),
                button: rgb(0xffffff),
                button_border: rgb(0xc6c6c6),
                hover: rgb(0xebebeb),
                pressed: rgb(0xdedede),
                tool_active: rgb(0xe2e2e2),
                text: rgb(0x161616),
                text_label: rgb(0x343434),
                text_dim: rgb(0x707070),
                text_disabled: rgb(0xababab),
                icon: rgb(0x585858),
                track: rgb(0xbdbdbd),
                thumb: rgb(0x606060),
                thumb_hover: rgb(0x202020),
                accent: rgb(0x0a62d0),
                star: rgb(0x3a3a3a),
                pick: rgb(0x2a2a2a),
                reject: rgb(0xc83434),
                caution: rgb(0xb0761a),
                ..base
            },
            ThemeKind::StudioDark => return Tokens::studio(true),
            ThemeKind::StudioLight => return Tokens::studio(false),
        };
        // the bars are the panels' colour: one surface, flush
        Tokens { backdrop: tokens.chrome, ..tokens }
    }

    /// The Studio design, after the server's admin page (`apps/lightcraft-server/admin/admin.css`):
    /// its neutral greys (a backdrop, white or near-black cards on it, a lighter fill for buttons
    /// and fields), its blue accent, capsule buttons, rounded fields, cards, menus and dialogs, and
    /// filled sliders. The photo's own canvas stays a quiet neutral so colours are judged fairly.
    fn studio(dark: bool) -> Tokens {
        let rgb = |v: u32| Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
        let shape = Tokens {
            card_radius: 12.0,
            dock_gap: 8.0,
            control_radius: 6,
            button_radius: 255,
            window_radius: 14,
            filled_sliders: true,
            strip_w: 52.0,
            ..Tokens::default()
        };
        if dark {
            Tokens {
                backdrop: rgb(0x111111),
                chrome: rgb(0x1e1e1e),
                canvas: rgb(0x1b1b1b),
                grid_bg: rgb(0x1e1e1e),
                cell: rgb(0x262626),
                cell_selected: rgb(0x283766),
                divider: rgb(0x111111),
                inset: rgb(0x262626),
                field: rgb(0x161616),
                field_border: rgb(0x444444),
                button: rgb(0x2c2c2c),
                button_border: rgb(0x2c2c2c),
                hover: rgb(0x393939),
                pressed: rgb(0x444444),
                tool_active: rgb(0x323232),
                text: rgb(0xf2f2f2),
                text_label: rgb(0xdbdbdb),
                text_dim: rgb(0x8f8f8f),
                text_disabled: rgb(0x575757),
                icon: rgb(0xafafaf),
                track: rgb(0x444444),
                thumb: rgb(0xdbdbdb),
                thumb_hover: rgb(0xffffff),
                accent: rgb(0x4f74fc),
                star: rgb(0xdbdbdb),
                pick: rgb(0xf2f2f2),
                reject: rgb(0xe0412d),
                caution: rgb(0xf68511),
                switch_off: rgb(0x444444),
                switch_on: rgb(0x4f74fc),
                segment_pill: rgb(0x444444),
                scrim: 153,
                ..shape
            }
        } else {
            Tokens {
                backdrop: rgb(0xebebeb),
                chrome: rgb(0xffffff),
                canvas: rgb(0xf7f7f7),
                grid_bg: rgb(0xffffff),
                cell: rgb(0xf3f3f3),
                cell_selected: rgb(0xdfe7ff),
                divider: rgb(0xebebeb),
                inset: rgb(0xf3f3f3),
                field: rgb(0xffffff),
                field_border: rgb(0xd5d5d5),
                button: rgb(0xebebeb),
                button_border: rgb(0xebebeb),
                hover: rgb(0xe1e1e1),
                pressed: rgb(0xd5d5d5),
                tool_active: rgb(0xe9e9e9),
                text: rgb(0x131313),
                text_label: rgb(0x292929),
                text_dim: rgb(0x6e6e6e),
                text_disabled: rgb(0xb1b1b1),
                icon: rgb(0x505050),
                track: rgb(0xdadada),
                thumb: rgb(0x3d3d3d),
                thumb_hover: rgb(0x131313),
                accent: rgb(0x3b63fb),
                star: rgb(0x2c2c2c),
                pick: rgb(0x131313),
                reject: rgb(0xd73220),
                caution: rgb(0xc95f00),
                switch_off: rgb(0xd5d5d5),
                switch_on: rgb(0x3b63fb),
                segment_pill: rgb(0xffffff),
                scrim: 102,
                ..shape
            }
        }
    }

    /// Whether docked panels are cards floating on the backdrop (the Studio themes).
    pub fn cards(&self) -> bool {
        self.card_radius > 0.0
    }

    pub fn get(ctx: &egui::Context) -> Tokens {
        ctx.data(|d| d.get_temp::<Tokens>(egui::Id::NULL)).unwrap_or_default()
    }
    pub fn font(&self, size: f32) -> FontId {
        FontId::proportional(size)
    }
    pub fn semibold(&self, size: f32) -> FontId {
        FontId::new(size, FontFamily::Name(FONT_SEMIBOLD.into()))
    }
}

pub fn install_fonts(ctx: &egui::Context) {
    install_fonts_with_chinese(ctx, None);
}

/// Install the browser's separately downloaded Chinese face alongside the embedded faces.
pub fn install_fonts_with_chinese(ctx: &egui::Context, chinese: Option<&Arc<FontData>>) {
    ctx.set_fonts(font_definitions_with_chinese(lightcraft_engine::CRAFT_FONTS, chinese));
}

/// Inter remains the default UI face. Egui's default faces and craft-fonts cover other scripts;
/// the browser's separate Chinese face precedes Japanese faces within the CJK fallback list.
/// Without craft-fonts or that separate face, CJK text shows boxes.
pub fn font_definitions(craft: &'static [lightcraft_engine::CraftFont]) -> FontDefinitions {
    font_definitions_with_chinese(craft, None)
}

/// The web build keeps the large Simplified Chinese face outside the size-limited WASM module.
/// Prefer it for Chinese UI text, including Traditional Chinese until it has its own face.
pub fn font_definitions_with_chinese(craft: &'static [lightcraft_engine::CraftFont], chinese: Option<&Arc<FontData>>) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("Inter".into(), Arc::new(FontData::from_static(include_bytes!("../../../assets/fonts/Inter-Regular.ttf"))));
    fonts.font_data.insert("Inter-SemiBold".into(), Arc::new(FontData::from_static(include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf"))));
    // Craft-fonts faces in preference order for a family drawn in `style`.
    let fallback = |style: &str| {
        lightcraft_engine::fonts::ui_fallback(craft, crate::i18n::language().script(), style)
            .into_iter()
            .map(craft_font_name)
            .collect::<Vec<String>>()
    };
    let (mut regular, mut bold) = (fallback("Regular"), fallback("Bold"));
    for name in regular.iter().chain(bold.iter()) {
        if let Some(font) = craft.iter().find(|font| &craft_font_name(font) == name) {
            fonts.font_data.insert(name.clone(), Arc::new(FontData::from_static(font.bytes)));
        }
    }
    let script = crate::i18n::language().script();
    let prefer_chinese = chinese.is_some() && matches!(script, "Hans" | "Hant");
    if let Some(font) = chinese {
        fonts.font_data.insert(WEB_CHINESE_FONT.into(), font.clone());
        if prefer_chinese {
            regular.insert(0, WEB_CHINESE_FONT.into());
            bold.insert(0, WEB_CHINESE_FONT.into());
        } else {
            regular.push(WEB_CHINESE_FONT.into());
            bold.push(WEB_CHINESE_FONT.into());
        }
    }
    let defaults: Vec<String> = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut prop = vec!["Inter".to_string()];
    prop.extend(defaults.iter().cloned());
    prop.extend(regular);
    fonts.families.insert(FontFamily::Proportional, prop);
    let mut semi = vec!["Inter-SemiBold".to_string()];
    semi.extend(defaults);
    semi.extend(bold);
    fonts.families.insert(FontFamily::Name(FONT_SEMIBOLD.into()), semi);
    let mono = fonts.families.entry(FontFamily::Monospace).or_default();
    let mut mono_fallback = fallback("Regular");
    if prefer_chinese {
        mono_fallback.insert(0, WEB_CHINESE_FONT.into());
    }
    if chinese.is_some() && !prefer_chinese {
        mono_fallback.push(WEB_CHINESE_FONT.into());
    }
    mono.extend(mono_fallback);
    fonts
}

/// The font families used by this app instance, including the browser's separately loaded face.
pub fn font_credits(chinese_loaded: bool) -> String {
    let mut families = vec!["Inter"];
    for f in lightcraft_engine::CRAFT_FONTS {
        if !families.contains(&f.family) {
            families.push(f.family);
        }
    }
    if chinese_loaded && !families.contains(&"Noto Sans CJK SC") {
        families.push("Noto Sans CJK SC");
    }
    families.join(" / ")
}

fn craft_font_name(f: &lightcraft_engine::CraftFont) -> String {
    format!("craft-fonts {} {}", f.family, f.style)
}

fn theme_id() -> egui::Id {
    egui::Id::new("lightcraft-theme")
}

/// Whether the compact look is applied, and in which appearance (dark).
fn compact_id() -> egui::Id {
    egui::Id::new("lc-styled")
}

/// The theme `ctx` is styled with (none before the first [`apply_kind`]).
pub fn current(ctx: &egui::Context) -> Option<ThemeKind> {
    ctx.data(|d| d.get_temp::<ThemeKind>(theme_id()))
}

/// Style `ctx` with the default theme.
pub fn apply(ctx: &egui::Context) {
    apply_kind(ctx, ThemeKind::default());
}

/// Style `ctx` with `kind` (the desktop layout). The visuals go to both of egui's styles, so egui
/// following the system's dark/light setting on its own never swaps in its stock look.
pub fn apply_kind(ctx: &egui::Context, kind: ThemeKind) {
    let t = Tokens::for_kind(kind);
    let dark = kind.is_dark();
    ctx.data_mut(|d| {
        d.insert_temp(theme_id(), kind);
        d.remove::<bool>(compact_id());
    });
    apply_tokens(ctx, t, dark);
}

/// The compact layout's look: iOS's (`Tokens::ios_for`, larger type, borderless rounded controls,
/// 44 pt rows) in its light or dark appearance. Cheap to call every frame: it only restyles when
/// the appearance changes or the desktop theme was applied in between.
pub fn apply_compact(ctx: &egui::Context, dark: bool) {
    if ctx.data(|d| d.get_temp::<bool>(compact_id())) == Some(dark) {
        return;
    }
    ctx.data_mut(|d| {
        // none of the desktop themes is shown: the next `apply_kind` restyles
        d.remove::<ThemeKind>(theme_id());
        d.insert_temp(compact_id(), dark);
    });
    let t = Tokens::ios_for(dark);
    apply_tokens(ctx, t, dark);
    ctx.all_styles_mut(|s| {
        let v = &mut s.visuals;
        v.window_corner_radius = CornerRadius::same(13);
        v.menu_corner_radius = CornerRadius::same(13);
        v.window_stroke = Stroke::NONE;
        v.override_text_color = Some(t.text);
        let w = &mut v.widgets;
        for wv in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            wv.corner_radius = CornerRadius::same(9);
            wv.fg_stroke = Stroke::new(1.0, t.text);
        }
        w.inactive.bg_stroke = Stroke::NONE;
        w.hovered.bg_stroke = Stroke::NONE;
        w.active.bg_stroke = Stroke::NONE;
        w.noninteractive.bg_stroke = Stroke::new(1.0, t.divider);
        s.spacing.item_spacing = egui::vec2(8.0, 8.0);
        s.spacing.button_padding = egui::vec2(14.0, 8.0);
        s.spacing.interact_size.y = crate::TOUCH_ROW_H;
        s.text_styles.insert(egui::TextStyle::Body, FontId::proportional(16.0));
        s.text_styles.insert(egui::TextStyle::Button, FontId::proportional(16.0));
        s.text_styles.insert(egui::TextStyle::Small, FontId::proportional(13.0));
        s.text_styles.insert(egui::TextStyle::Heading, FontId::new(17.0, FontFamily::Name(FONT_SEMIBOLD.into())));
        // (panels slide, switches and tiles fade over this: iOS's quick-but-visible quarter second)
        s.animation_time = 0.25;
    });
}

/// The look both layouts share: `t`'s colours on egui's dark or light visuals, the desktop's metrics.
fn apply_tokens(ctx: &egui::Context, t: Tokens, dark: bool) {
    // Keep photo zoom shortcuts from resizing egui itself.
    ctx.options_mut(|o| o.zoom_with_keyboard = false);
    ctx.data_mut(|d| {
        d.insert_temp(egui::Id::NULL, t);
        d.insert_temp(egui::Id::new("lc-dark"), dark);
    });
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = t.chrome;
    v.window_fill = t.chrome;
    v.extreme_bg_color = t.field;
    v.faint_bg_color = t.inset;
    v.window_stroke = Stroke::new(1.0, t.button_border);
    v.window_corner_radius = CornerRadius::same(t.window_radius);
    // (a menu is smaller than a dialog: a little less round)
    v.menu_corner_radius = CornerRadius::same(t.window_radius.min(10));
    v.selection.bg_fill = t.accent.gamma_multiply(0.6);
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.override_text_color = Some(t.text_label);
    let (popup, window) = if dark { (140, 160) } else { (50, 60) };
    v.popup_shadow = egui::epaint::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(popup) };
    v.window_shadow = egui::epaint::Shadow { offset: [0, 8], blur: 30, spread: 0, color: Color32::from_black_alpha(window) };
    let w = &mut v.widgets;
    craft_ui::theme::WidgetPalette { noninteractive: t.chrome, inactive: t.button, hovered: t.hover, active: t.pressed, open: t.hover }.apply_to(
        w,
        CornerRadius::same(t.control_radius),
        Stroke::new(1.0, t.text_label),
    );
    w.noninteractive.bg_stroke = Stroke::new(1.0, t.divider);
    w.inactive.bg_stroke = Stroke::new(1.0, t.button_border);
    if t.cards() {
        // (egui fills a focused window's title bar with this: Studio's dialogs are one surface,
        // their title on it as on the admin page's)
        w.open.weak_bg_fill = t.chrome;
    }
    ctx.set_visuals_of(egui::Theme::Dark, v.clone());
    ctx.set_visuals_of(egui::Theme::Light, v);
    ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        // (capsule buttons need a little more room at their ends)
        s.spacing.button_padding = egui::vec2(if t.button_radius > 12 { 12.0 } else { 8.0 }, 3.0);
        s.spacing.interact_size.y = 22.0;
        s.text_styles.insert(egui::TextStyle::Body, FontId::proportional(13.0));
        s.text_styles.insert(egui::TextStyle::Button, FontId::proportional(13.0));
        s.text_styles.insert(egui::TextStyle::Small, FontId::proportional(11.0));
        s.text_styles.insert(egui::TextStyle::Heading, FontId::new(16.0, FontFamily::Name(FONT_SEMIBOLD.into())));
        s.animation_time = 0.08;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{Locale, set_language};

    /// #736: a Thai folder name drew boxes because only CJK craft-fonts faces were fallbacks. With a
    /// craft-fonts build that carries a Thai face, an English UI offers it in every family.
    #[test]
    fn thai_face_is_a_fallback_in_every_ui_family_when_craft_fonts_carry_one() {
        let Some(thai) = lightcraft_engine::CRAFT_FONTS.iter().find(|f| f.covers("Thai")) else {
            eprintln!("no Thai face in craft-fonts: nothing to check");
            return;
        };
        set_language(Locale::En);
        let definitions = font_definitions(lightcraft_engine::CRAFT_FONTS);
        let name = craft_font_name(thai);
        assert!(definitions.font_data.contains_key(&name));
        for family in [FontFamily::Proportional, FontFamily::Name(FONT_SEMIBOLD.into()), FontFamily::Monospace] {
            assert!(definitions.families.get(&family).is_some_and(|names| names.contains(&name)), "{family:?} falls back on {name}");
        }
    }

    #[test]
    fn native_chinese_face_falls_back_after_default_faces() {
        static FACES: &[lightcraft_engine::CraftFont] = &[
            lightcraft_engine::CraftFont {
                family: "BIZ UDPGothic",
                style: "Regular",
                scripts: &["Jpan", "Latn"],
                bytes: include_bytes!("../../../assets/fonts/Inter-Regular.ttf"),
            },
            lightcraft_engine::CraftFont {
                family: "Noto Sans CJK SC",
                style: "Regular",
                scripts: &["Hans", "Latn"],
                bytes: include_bytes!("../../../assets/fonts/Inter-Regular.ttf"),
            },
        ];
        set_language(Locale::ZhHans);
        let definitions = font_definitions(FACES);
        let defaults = FontDefinitions::default();
        let default_prop = defaults.families.get(&FontFamily::Proportional).expect("default proportional fonts");
        let chinese = "craft-fonts Noto Sans CJK SC Regular";
        for (family, primary) in [(FontFamily::Proportional, "Inter"), (FontFamily::Name(FONT_SEMIBOLD.into()), "Inter-SemiBold")] {
            let names = definitions.families.get(&family).expect("UI font family");
            assert_eq!(names.first().map(String::as_str), Some(primary));
            assert_eq!(names.get(1), default_prop.first());
            assert!(names.iter().position(|name| name == chinese).is_some_and(|position| position > default_prop.len()));
        }
        let mono = definitions.families.get(&FontFamily::Monospace).expect("monospace font family");
        let default_mono = defaults.families.get(&FontFamily::Monospace).expect("default monospace fonts");
        assert_eq!(mono.first(), default_mono.first());
        assert!(mono.iter().position(|name| name == chinese).is_some_and(|position| position >= default_mono.len()));
        set_language(Locale::En);
    }

    #[test]
    fn separate_web_chinese_face_is_available_in_every_ui_family() {
        // Simulate the WASM build, which embeds only the Japanese face. The Chinese face arrives
        // as a separate file and must precede that face when the UI uses Chinese.
        static JAPANESE: &[lightcraft_engine::CraftFont] = &[lightcraft_engine::CraftFont {
            family: "BIZ UDPGothic",
            style: "Regular",
            scripts: &["Jpan", "Latn"],
            bytes: include_bytes!("../../../assets/fonts/Inter-Regular.ttf"),
        }];
        let embedded = lightcraft_engine::CRAFT_FONTS.iter().find(|font| font.family == "Noto Sans CJK SC" && font.style == "Regular");
        let fallback: &'static [u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
        let source = embedded.map_or(fallback, |font| font.bytes);
        let chinese = Arc::new(FontData::from_static(source));
        set_language(Locale::ZhHans);
        let definitions = font_definitions_with_chinese(JAPANESE, Some(&chinese));
        let default_count = FontDefinitions::default().families.get(&FontFamily::Proportional).map_or(0, Vec::len);
        for (family, primary) in [(FontFamily::Proportional, "Inter"), (FontFamily::Name(FONT_SEMIBOLD.into()), "Inter-SemiBold")] {
            let names = definitions.families.get(&family).expect("UI font family");
            assert_eq!(names.first().map(String::as_str), Some(primary));
            assert_eq!(names.get(1 + default_count).map(String::as_str), Some(WEB_CHINESE_FONT));
        }
        assert!(definitions.families.get(&FontFamily::Monospace).is_some_and(|names| names.contains(&WEB_CHINESE_FONT.to_string())));

        if embedded.is_some() {
            let ctx = egui::Context::default();
            ctx.set_fonts(definitions);
            let mut frame = ctx.run_ui(egui::RawInput::default(), |_| {});
            frame.textures_delta.clear();
            ctx.fonts_mut(|fonts| {
                for family in [FontFamily::Proportional, FontFamily::Name(FONT_SEMIBOLD.into()), FontFamily::Monospace] {
                    let id = FontId::new(13.0, family);
                    assert!("简体中文字".chars().all(|ch| fonts.has_glyph(&id, ch)), "{id:?}");
                }
            });
        }

        set_language(Locale::Ja);
        let japanese = font_definitions_with_chinese(JAPANESE, Some(&chinese));
        let names = japanese.families.get(&FontFamily::Proportional).expect("proportional fonts");
        assert_eq!(names.get(1 + default_count).map(String::as_str), Some("craft-fonts BIZ UDPGothic Regular"));
        assert_eq!(names.get(2 + default_count).map(String::as_str), Some(WEB_CHINESE_FONT));
        set_language(Locale::En);
    }
}
