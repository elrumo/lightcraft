use super::*;

/// `path` shortened to fit `max` (as `width` measures it) by dropping leading folders, after
/// either kind of separator, so that its end stays (`…/Pictures/Library`,
/// `…\Pictures\Library`): the end says which folder it is. A last folder too long by itself
/// loses its end ([`crate::widgets::elide_head`]).
pub(super) fn elide_path(path: &str, max: f32, width: impl Fn(&str) -> f32) -> String {
    if width(path) <= max {
        return path.to_string();
    }
    // (a separator at the very end is not where a name starts: `…/` alone would say nothing)
    let end = path.trim_end_matches(['/', '\\']).len();
    let mut rest = path;
    while let Some(i) = rest.find(['/', '\\']).filter(|i| path.len() - rest.len() + i + 1 < end) {
        let separator = if rest.get(i..).is_some_and(|r| r.starts_with('\\')) { '\\' } else { '/' };
        rest = rest.get(i + 1..).unwrap_or("");
        let shorter = format!("…{separator}{rest}");
        if width(&shorter) <= max {
            return shorter;
        }
    }
    let name = rest.trim_end_matches(['/', '\\']);
    // (nothing but separators: never an empty label)
    if name.is_empty() { "…".to_string() } else { crate::widgets::elide_head(name, max, width) }
}

/// A folder or file path on one line in the room left in its row (`text:{id}`): shortened from
/// the front when it is too long ([`elide_path`]), with all of it in a tooltip then.
pub(crate) fn path_label(ui: &mut egui::Ui, id: &str, path: &str, font: egui::FontId, color: egui::Color32) {
    path_label_beside(ui, id, path, font, color, &[]);
}

/// [`path_label`] with room left after it for `beside`, the texts of the controls that follow it in
/// the row (their buttons are as wide as the text and the padding round it): a long path gives way
/// to them, so they stay in the row.
pub(crate) fn path_label_beside(ui: &mut egui::Ui, id: &str, path: &str, font: egui::FontId, color: egui::Color32, beside: &[&str]) {
    let pad = ui.spacing().button_padding.x * 2.0 + ui.spacing().item_spacing.x;
    let reserve: f32 = beside
        .iter()
        .map(|text| ui.painter().layout_no_wrap(text.to_string(), egui::TextStyle::Button.resolve(ui.style()), color).size().x + pad)
        .sum();
    let room = (ui.available_width() - reserve).max(24.0);
    let shown = elide_path(path, room, |s| ui.painter().layout_no_wrap(s.to_string(), font.clone(), color).size().x);
    // (`truncate` is the last resort, for a room narrower than "…")
    let r = ui.add(egui::Label::new(RichText::new(&shown).font(font).color(color)).truncate().show_tooltip_when_elided(false));
    register(ui, format!("text:{id}"), r.rect);
    if shown != path {
        r.on_hover_text(path);
    }
}

/// A desktop choice with more options than this is a combo box rather than a row of buttons.
pub(crate) const PICK_BUTTONS: usize = 6;

/// The desktop Settings: the sidebar's width, a row's height, how far a card reaches past the rows
/// on it (their text lines up with the headings above the cards).
pub(crate) const SIDEBAR_W: f32 = 188.0;
pub(crate) const DESK_ROW_H: f32 = 34.0;
pub(crate) const CARD_PAD: f32 = 12.0;

/// The desktop sidebar's groups of sections, a line between them: the library and how LightCraft
/// looks and works, then the AI features, then sync.
pub(crate) const SIDEBAR: [&[&str]; 3] =
    [&["general", "interface", "slideshow", "display", "import", "performance"], &["faces", "denoise", "models"], &["sync"]];

/// The iOS app's library lives in its container: no folder to choose or watch, no external editor,
/// no other app to read XMP sidecars, no Local folders. Those settings aren't offered there.
pub(crate) const IOS: bool = cfg!(target_os = "ios");

/// The phone's list: each tab's icon and the colour of its tile (iOS's system colours).
pub(crate) const TAB_TILES: [(Icon, Color32); 10] = [
    (Icon::Gear, Color32::from_rgb(0x8e, 0x8e, 0x93)),
    (Icon::Photos, Color32::from_rgb(0x30, 0xd1, 0x58)),
    (Icon::Wand, Color32::from_rgb(0xff, 0x9f, 0x0a)),
    (Icon::Monitor, Color32::from_rgb(0x64, 0xd2, 0xff)),
    (Icon::Eye, Color32::from_rgb(0x5e, 0x5c, 0xe6)),
    (Icon::Play, Color32::from_rgb(0xff, 0x45, 0x3a)),
    (Icon::FaceBox, Color32::from_rgb(0xff, 0x37, 0x5f)),
    (Icon::Detail, Color32::from_rgb(0xac, 0x8e, 0x68)),
    (Icon::Cloud, Color32::from_rgb(0x0a, 0x84, 0xff)),
    (Icon::Subject, Color32::from_rgb(0xbf, 0x5a, 0xf2)),
];

/// A row's height on the phone (Apple's 44 pt).
pub(crate) const ROW: f32 = crate::TOUCH_ROW_H;

/// The phone page's title, and its back button (`None` on the list).
pub fn phone_title(tab: &str) -> (&'static str, Option<&'static str>) {
    match TABS.iter().find(|(id, _)| *id == tab) {
        Some((_, label)) => (*label, Some("Settings")),
        None => ("Settings", None),
    }
}

/// The desktop Settings' height, given the room the dialog has for it (`max`): as much as fits, up
/// to a comfortable height (in a short window the sidebar scrolls too).
pub fn desktop_height(max: f32) -> f32 {
    if max.is_nan() { 620.0 } else { max.clamp(1.0, 620.0) }
}

pub(crate) fn heading(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    if crate::is_compact(ui.ctx()) {
        // iOS: small grey capitals over the card
        card_end(ui, t);
        ui.add_space(26.0);
        // (a heading with a long server address in it wraps like a note)
        egui::Frame::NONE.inner_margin(Margin::symmetric(16, 0)).show(ui, |ui| {
            ui.label(RichText::new(crate::i18n::tr(text).to_uppercase()).size(13.0).color(t.text_dim));
        });
        ui.add_space(7.0);
        return;
    }
    if let Some(col) = column(ui) {
        card_end(ui, t);
        // (a heading under the section's title needs no gap of its own)
        if ui.cursor().top() > col.top + 1.0 {
            ui.add_space(14.0);
        }
        ui.label(RichText::new(crate::i18n::tr(text)).font(t.semibold(13.5)).color(t.text));
        ui.add_space(6.0);
        return;
    }
    ui.add_space(4.0);
    ui.label(RichText::new(crate::i18n::tr(text)).font(t.semibold(12.5)).color(t.text));
}

/// A labelled row. On the phone: the label on the left and `add`'s widgets on the right, on the
/// card; without a label, `add`'s own rows (buttons, switches) as they are.
pub(crate) fn row<R>(ui: &mut egui::Ui, t: &Tokens, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    if crate::is_compact(ui.ctx()) {
        if label.is_empty() {
            return add(ui);
        }
        card_row(ui, t, 16.0);
        return egui::Frame::NONE
            .inner_margin(Margin::symmetric(16, 0))
            .show(ui, |ui| {
                ui.allocate_ui_with_layout(vec2(ui.available_width(), ROW), Layout::left_to_right(Align::Center), |ui| {
                    ui.set_min_height(ROW);
                    ui.label(RichText::new(crate::i18n::tr(label)).size(17.0).color(t.text));
                    ui.add_space(12.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), add).inner
                })
                .inner
            })
            .inner;
    }
    if column(ui).is_some() {
        // a Settings section's row, on its card: the name on the left, the controls at the end
        card_row(ui, t, 0.0);
        let w = ui.available_width();
        return ui
            .allocate_ui_with_layout(vec2(w, DESK_ROW_H), Layout::left_to_right(Align::Center), |ui| {
                ui.set_min_height(DESK_ROW_H);
                if !label.is_empty() {
                    ui.add(egui::Label::new(RichText::new(crate::i18n::tr(label)).color(t.text)).wrap_mode(egui::TextWrapMode::Extend));
                    ui.add_space(12.0);
                }
                trailing(ui, add)
            })
            .inner;
    }
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(egui::vec2(LABEL_W, 24.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.set_min_width(LABEL_W);
            // (a label longer than its column wraps in it; it does not push the controls along)
            ui.add(egui::Label::new(RichText::new(crate::i18n::tr(label)).color(t.text_label)).wrap());
        });
        // The controls wrap in the room beside the label, each further line starting under the
        // first control, so a row with many choices grows down and never widens the window.
        let wrapped = egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true);
        ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 24.0), wrapped, |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            add(ui)
        })
        .inner
    })
    .inner
}

/// `add`'s widgets at the end of the row, in the order they are added. Their width is measured and
/// kept for the next pass, with the room the row had then; a pass whose widths changed (the first
/// one, a row that grew) is discarded, so a row is never seen out of place. A row given more room
/// than when it was measured is measured afresh: otherwise a path elided to fit a narrow first
/// frame would stay short in a wide window.
pub(crate) fn trailing<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    // (the row's own id: rows without a name, or with the same one, are told apart)
    let id = ui.unique_id().with("trailing");
    let room = ui.available_width();
    let known = ui.data(|d| d.get_temp::<(f32, f32)>(id)).filter(|(was, _)| (was - room).abs() <= 0.5).map(|(_, used)| used);
    let width = known.unwrap_or(room).min(room);
    ui.add_space((room - width).max(0.0));
    let start = ui.cursor().left();
    let out = add(ui);
    let used = (ui.min_rect().right() - start).max(0.0);
    if known.is_none_or(|w| (w - used).abs() > 0.5) {
        ui.data_mut(|d| d.insert_temp(id, (room, used)));
        ui.ctx().request_discard("a Settings row's width");
    }
    out
}

/// A note. On the phone it goes under the card above it, as iOS's section footers do.
pub(crate) fn hint(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    if crate::is_compact(ui.ctx()) {
        card_end(ui, t);
        ui.add_space(7.0);
        egui::Frame::NONE.inner_margin(Margin::symmetric(16, 0)).show(ui, |ui| {
            ui.label(RichText::new(crate::i18n::tr(text)).size(13.0).color(t.text_dim));
        });
        ui.add_space(10.0);
        return;
    }
    if column(ui).is_some() {
        // under the row it explains, on its card; under a heading, for the whole group
        let on_card = ui.data(|d| d.get_temp::<Card>(card_id())).is_some();
        ui.label(RichText::new(crate::i18n::tr(text)).size(if on_card { 11.5 } else { 12.0 }).color(t.text_dim));
        ui.add_space(4.0);
        return;
    }
    ui.label(RichText::new(crate::i18n::tr(text)).size(11.0).color(t.text_dim));
}

/// [`hint`] on one line, cut with "…" and the whole text on hover: for text that can be long and
/// changes between frames (a driver's message), so nothing below it moves under the pointer.
pub(crate) fn hint_line(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    let compact = crate::is_compact(ui.ctx());
    if compact {
        card_end(ui, t);
        ui.add_space(7.0);
    }
    let label = egui::Label::new(RichText::new(crate::i18n::tr(text)).size(if compact { 13.0 } else { 11.5 }).color(t.text_dim)).truncate();
    ui.add(label).on_hover_text(text);
    ui.add_space(if compact { 10.0 } else { 4.0 });
}

/// A checkbox addressable as `check:{id}` (a row with a switch on the phone, and in a desktop
/// Settings section); true when toggled.
pub(crate) fn check(ui: &mut egui::Ui, id: &str, value: &mut bool, label: &str) -> bool {
    if column(ui).is_some() {
        let r = switch_row(ui, value, crate::i18n::tr(label));
        register(ui, format!("check:{id}"), r.rect);
        return r.changed();
    }
    let r = if crate::is_compact(ui.ctx()) {
        card_row(ui, &Tokens::get(ui.ctx()), 16.0);
        egui::Frame::NONE
            .inner_margin(Margin::symmetric(16, 0))
            .show(ui, |ui| crate::widgets::check(ui, value, RichText::new(crate::i18n::tr(label)).size(17.0)))
            .inner
    } else {
        crate::widgets::check(ui, value, crate::i18n::tr(label))
    };
    register(ui, format!("check:{id}"), r.rect);
    r.changed()
}

/// A desktop Settings row that is an on / off choice: its name (wrapping when long) on the left, a
/// switch on the right in the accent colour when on. The whole row takes the click.
pub(crate) fn switch_row(ui: &mut egui::Ui, on: &mut bool, label: &str) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    card_row(ui, &t, 0.0);
    let enabled = ui.is_enabled();
    let w = ui.available_width();
    let text = egui::WidgetText::from(RichText::new(label).color(if enabled { t.text } else { t.text_disabled }));
    let galley = text.into_galley(ui, Some(egui::TextWrapMode::Wrap), (w - 56.0).max(40.0), egui::TextStyle::Body);
    let (r, mut resp) = ui.allocate_exact_size(vec2(w, (galley.size().y + 14.0).max(DESK_ROW_H)), Sense::click());
    if resp.clicked() && enabled {
        *on = !*on;
        resp.mark_changed();
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, enabled, *on, galley.text()));
    let k = ui.ctx().animate_bool_with_time(resp.id.with("switch"), *on, 0.12);
    let p = ui.painter();
    p.galley(pos2(r.left(), r.center().y - galley.size().y / 2.0), galley, t.text);
    let sw = Rect::from_min_size(pos2(r.right() - 34.0, r.center().y - 10.0), vec2(34.0, 20.0));
    let track = crate::widgets::lerp(t.track, t.accent, k);
    p.rect_filled(sw, 10.0, if enabled { track } else { track.gamma_multiply(0.5) });
    if resp.hovered() && enabled {
        p.rect_filled(sw, 10.0, Color32::from_white_alpha(18));
    }
    p.circle_filled(pos2(sw.left() + 10.0 + (sw.width() - 20.0) * k, sw.center().y), 8.0, Color32::WHITE);
    resp
}

/// Mutually exclusive buttons (`button:{id}-{index}`).
pub(crate) fn choices<V: PartialEq + Copy>(ui: &mut egui::Ui, id: &str, options: &[(V, &str)], value: &mut V) -> bool {
    let mut changed = false;
    ui.spacing_mut().item_spacing.x = 4.0;
    for (i, (v, l)) in options.iter().enumerate() {
        if crate::widgets::choice_button(ui, &format!("{id}-{i}"), l, *value == *v).clicked() && *value != *v {
            *value = *v;
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod path_tests {
    use super::elide_path;

    /// Width = characters, so a limit of 20 is "20 characters".
    fn fit(path: &str, max: usize) -> String {
        elide_path(path, max as f32, |s| s.chars().count() as f32)
    }

    #[test]
    fn a_path_that_fits_is_left_alone() {
        assert_eq!(fit("/Users/me/Pictures", 18), "/Users/me/Pictures");
        assert_eq!(fit("", 5), "");
    }

    #[test]
    fn leading_folders_go_first_and_the_end_stays() {
        assert_eq!(fit("/Users/me/Pictures/Library", 20), "…/Pictures/Library");
        assert_eq!(fit("/Users/me/Pictures/Library", 10), "…/Library");
    }

    /// Windows paths: `Path::display` gives backslashes there.
    #[test]
    fn a_path_with_backslashes_keeps_its_end_too() {
        assert_eq!(fit(r"C:\Users\me\Pictures\Library", 20), r"…\Pictures\Library");
        assert_eq!(fit(r"C:\Users\me/Pictures\Library", 10), r"…\Library");
    }

    #[test]
    fn a_last_folder_too_long_by_itself_loses_its_end() {
        assert_eq!(fit("/Users/me/a-very-long-folder-name", 8), "a-very-…");
        assert_eq!(fit("no-separator-at-all", 8), "no-sepa…");
        assert_eq!(fit("/Users/Фотографии-из-отпуска", 8), "Фотогра…");
    }

    #[test]
    fn a_separator_at_the_end_is_not_where_the_name_starts() {
        assert_eq!(fit("/Users/me/Pictures/Library/", 11), "…/Library/");
        assert_eq!(fit("/Users/me/a-very-long-folder-name/", 8), "a-very-…");
        assert_eq!(fit(r"C:\Users\me\Library\", 11), r"…\Library\");
        assert_eq!(fit("///", 2), "…");
        assert_eq!(fit(r"\\server\share\folder", 15), r"…\share\folder");
    }
}