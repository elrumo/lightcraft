//! Lightroom-style widgets: sliders with gradient tracks and hollow thumbs, section headers,
//! flyout rows, bordered buttons. Every interactive widget registers an automation id + rect
//! ([`register`]) so the control channel can find and click it by name.

use egui::{Align2, Color32, CornerRadius, Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use lightcraft_develop::{ControlSpec, Track};

use crate::icons::{Icon, paint};
use crate::theme::Tokens;

/// Standard dropdown chevrons, including settings, import/export and rule editors.
pub fn combo_box(id: impl egui::AsIdSalt) -> egui::ComboBox {
    egui::ComboBox::from_id_salt(id).icon(|ui, rect, visuals, _| {
        paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(16.0, 16.0)), Icon::ChevronDown, visuals.fg_stroke.color);
    })
}

/// A native button with a vector arrow beside its text; egui reserves the icon's space.
pub fn arrow_button<'a>(ui: &mut Ui, text: impl egui::IntoAtoms<'a>, icon: Icon, leading: bool) -> Response {
    let id = ui.next_auto_id().with("arrow");
    let mut atoms = text.into_atoms();
    let arrow = egui::Atom::custom(id, vec2(16.0, 16.0));
    if leading {
        atoms.push_left(arrow);
    } else {
        atoms.push_right(arrow);
    }
    let r = egui::Button::new(atoms).atom_ui(ui);
    if let Some(rect) = r.rect(id) {
        paint(ui.painter(), rect, icon, ui.style().interact(&r.response).fg_stroke.color);
    }
    r.response
}

/// Keep egui's submenu interaction and layout, using our vector chevron instead of a font glyph.
pub fn menu_button<'a, R>(ui: &mut Ui, atoms: impl egui::IntoAtoms<'a>, contents: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<Option<R>> {
    if !egui::containers::menu::is_in_menu(ui) {
        return ui.menu_button(atoms, contents);
    }
    let button = egui::Button::new(atoms.into_atoms()).right_text("    ");
    let (response, inner) = egui::containers::menu::SubMenuButton::from_button(button).ui(ui, contents);
    let center = pos2(response.rect.right() - ui.spacing().button_padding.x - 8.0, response.rect.center().y);
    paint(ui.painter(), Rect::from_center_size(center, vec2(16.0, 16.0)), Icon::ChevronRight, ui.style().interact(&response).fg_stroke.color);
    egui::InnerResponse::new(inner.map(|r| r.inner), response)
}

// ------------------------------------------------------------------ automation registry

#[derive(Clone, Default)]
pub struct Registry(pub Vec<(String, Rect)>);

/// Record a widget's automation id and screen rect for this frame.
pub fn register(ctx: &egui::Context, id: impl Into<String>, rect: Rect) {
    let id = id.into();
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Registry>(egui::Id::new("lc-registry")).0.push((id, rect)));
}

/// Esc was handled this frame by something nested (an open popup closed on it): containers such
/// as dialogs shouldn't close too. Keyed to the frame, so it lapses by itself on the next one.
pub fn take_escape(ctx: &egui::Context) {
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("lc-escape-taken"), frame));
}

/// Whether [`take_escape`] was called this frame.
pub fn escape_taken(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<u64>(egui::Id::new("lc-escape-taken"))) == Some(ctx.cumulative_frame_nr())
}

pub fn take_registry(ctx: &egui::Context) -> Vec<(String, Rect)> {
    ctx.data_mut(|d| std::mem::take(&mut d.get_temp_mut_or_default::<Registry>(egui::Id::new("lc-registry")).0))
}

// ------------------------------------------------------------------ preview-only raws

/// The decoder's reason why a raw is shown from its embedded preview, shortened for the UI
/// ("Nikon Huffman-compressed NEF (no clean-room …)" → "Nikon Huffman-compressed NEF").
pub fn preview_only_variant(reason: &str) -> &str {
    reason.split(" (").next().unwrap_or(reason).trim()
}

/// What a preview-only raw means for the user (see `Photo::preview_only`).
pub fn preview_only_explanation(reason: &str) -> String {
    if reason.starts_with("Nikon High Efficiency NEF (") {
        return crate::i18n::tr(
            "Nikon High Efficiency NEF (HE / HE★) uses intoPIX TicoRAW compression. LightCraft does not currently have a license for this codec, so only the camera's embedded JPEG preview is available, not the raw sensor data. To edit raw data, shoot Lossless compressed NEF or convert existing HE files to DNG using a licensed converter.",
        )
        .to_string();
    }
    crate::i18n::tr_format!(
        "LightCraft can't decode this raw variant yet ({}). You're editing the camera's embedded JPEG preview, which already includes the camera's picture style (e.g. Monochrome) and white balance.",
        preview_only_variant(reason)
    )
}

/// A panel notice for a raw shown from its embedded preview (Edit, Info): an amber info icon,
/// "Preview only", the explanation and a Reload from Disk button (`button:reloadPreviewOnly:{key}`:
/// for a file a newer version can decode that was offline when its library opened). Registered as
/// `notice:previewOnly:{key}`. `true` when the button was clicked.
pub fn preview_only_notice(ui: &mut Ui, key: &str, reason: &str) -> bool {
    let mut reload = false;
    let t = Tokens::get(ui.ctx());
    let r = egui::Frame::NONE
        .fill(t.canvas)
        .stroke(Stroke::new(1.0, t.caution.gamma_multiply(0.45)))
        .corner_radius(6.0)
        .inner_margin(egui::Margin { left: 10, right: 10, top: 8, bottom: 9 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (ir, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                paint(ui.painter(), ir, Icon::Info, t.caution);
                ui.label(egui::RichText::new(crate::i18n::tr("Preview only")).font(t.semibold(12.5)).color(t.text));
            });
            ui.add_space(2.0);
            ui.label(egui::RichText::new(preview_only_explanation(reason)).size(11.5).color(t.text_label));
            ui.add_space(6.0);
            reload = text_button(ui, &format!("reloadPreviewOnly:{key}"), "Reload from Disk", false).clicked();
        })
        .response;
    let r = r.on_hover_text(crate::i18n::tr_format!("Decoder: {reason}", reason = reason));
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, crate::i18n::tr("Preview only: editing the camera's embedded JPEG")));
    register(ui.ctx(), format!("notice:previewOnly:{key}"), r.rect);
    reload
}

// ------------------------------------------------------------------ colour helpers

pub fn hex(s: &str) -> Color32 {
    let v = u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap_or(0x808080);
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

/// Band colours for the 8 colour-mixer bands (swatches and tracks).
pub const BAND_COLORS: [&str; 8] = ["#aa0000", "#bb6600", "#aaaa00", "#009900", "#00aaaa", "#0044cc", "#7722aa", "#aa0077"];

fn track_stops(track: &Track) -> Option<Vec<Color32>> {
    Some(match track {
        Track::Plain | Track::Centered => return None,
        Track::Temp => ["#193961", "#47566a", "#60666d", "#70705b", "#737339", "#75742a"].iter().map(|h| hex(h)).collect(),
        Track::Tint => ["#246123", "#2c552c", "#394739", "#4a3d4c", "#6b336b", "#872c87"].iter().map(|h| hex(h)).collect(),
        Track::Gradient { from, to } => vec![hex(from), hex(to)],
        Track::Hue { band } => {
            let i = *band as usize;
            let prev = hex(BAND_COLORS[(i + 7) % 8]);
            let next = hex(BAND_COLORS[(i + 1) % 8]);
            let me = hex(BAND_COLORS[i]);
            vec![
                prev.gamma_multiply(0.7),
                lerp(prev, me, 0.5).gamma_multiply(0.7),
                me.gamma_multiply(0.7),
                lerp(me, next, 0.5).gamma_multiply(0.7),
                next.gamma_multiply(0.7),
            ]
        }
        Track::Sat { band } => vec![hex("#454343"), lerp(hex("#454343"), hex(BAND_COLORS[*band as usize]), 0.6), hex(BAND_COLORS[*band as usize])],
        Track::Lum { band } => {
            let c = hex(BAND_COLORS[*band as usize]);
            vec![c.gamma_multiply(0.2), c.gamma_multiply(0.55), lerp(c, Color32::WHITE, 0.35), lerp(c, Color32::WHITE, 0.6)]
        }
        Track::Rainbow => ["#aa0000", "#aaaa00", "#009900", "#00aaaa", "#0044cc", "#aa0077", "#aa0000"].iter().map(|h| hex(h)).collect(),
    })
}

fn paint_track(ui: &Ui, rect: Rect, track: &Track, t: &Tokens, thumb_x: f32, ring: f32) {
    let p = ui.painter();
    let y = rect.center().y;
    let h = 1.0;
    if let Some(stops) = track_stops(track) {
        // gradient: segments between stops
        let n = stops.len().max(2) - 1;
        let steps = 48;
        for i in 0..steps {
            let (a, b) = (i as f32 / steps as f32, (i + 1) as f32 / steps as f32);
            let seg = (a * n as f32).floor() as usize;
            let local = a * n as f32 - seg as f32;
            let c = lerp(stops[seg.min(n)], stops[(seg + 1).min(n)], local);
            let x0 = rect.left() + rect.width() * a;
            let x1 = rect.left() + rect.width() * b;
            p.rect_filled(Rect::from_min_max(pos2(x0, y - h - 0.5), pos2(x1 + 0.5, y + h + 0.5)), 0.0, c);
        }
    } else {
        p.rect_filled(Rect::from_min_max(pos2(rect.left(), y - h), pos2(rect.right(), y + h)), 1.0, t.track);
    }
    // gap around the hollow thumb
    p.rect_filled(Rect::from_center_size(pos2(thumb_x, y), vec2(ring * 2.0 + 4.0, 6.0)), 0.0, t.chrome);
}

/// Result of a slider interaction this frame.
#[derive(Default, Debug, Clone, Copy)]
pub struct SliderOut {
    pub value: Option<f64>,
    pub drag_started: bool,
    pub drag_stopped: bool,
    pub reset: bool,
}

/// A slider row's height on a phone: the label and value, and the track under them.
pub const TOUCH_SLIDER_ROW_H: f32 = 46.0;

/// The left and right padding of what a tool panel holds: 24 and 22 pt on the desktop, 16 pt on a
/// phone (iOS's own margin).
pub fn side_pad(ctx: &egui::Context) -> (f32, f32) {
    if crate::is_compact(ctx) { (16.0, 16.0) } else { (24.0, 22.0) }
}

/// A tool panel's margin: the usual sides ([`side_pad`]) and `top` / `bottom` as given.
pub fn margin(ctx: &egui::Context, top: i8, bottom: i8) -> egui::Margin {
    let (left, right) = side_pad(ctx);
    egui::Margin { left: left as i8, right: right as i8, top, bottom }
}

/// What a value typed into a slider asks for (issue #322): a number such as `0.5`, `+12`, `-1,5`
/// (comma or point) or `5600 K`, on the control's steps and clamped to its range. `None` when it
/// isn't a finite number, so the slider keeps its value.
pub fn typed_value(spec: &ControlSpec, text: &str) -> Option<f64> {
    let t = text.trim().replace(',', ".");
    let t = t.trim_end_matches(|c: char| c == '%' || c == 'K' || c == 'k' || c.is_whitespace());
    let v: f64 = t.parse().ok().filter(|v: &f64| v.is_finite())?;
    let step = spec.step.max(1e-9);
    Some(((v / step).round() * step).clamp(spec.min, spec.max))
}

/// A slider's value as shown next to its label.
pub(crate) fn shown_value(spec: &ControlSpec, v: f64) -> String {
    let shown = if spec.id == "wb.temp" { format!("{v:.0}") } else { spec.format(v).replace("+0.00", "0").replace("-0.00", "0") };
    if shown == "+0" || shown == "-0" { "0".to_string() } else { shown }
}

/// Pointer travel (points, sideways from the press) that turns a press on a slider's value into a
/// scrub rather than a click that types a value.
const SCRUB_START: f32 = 3.0;

/// The radius of a slider's hollow ring thumb.
const THUMB_RING: f32 = 7.0;

/// A Lightroom slider row (label + value above a track with a hollow ring thumb).
/// Double-click the label or thumb to reset (hovering either says so). Shift-drag = fine adjustment.
/// Scroll sideways over it to nudge it. Click the value to type one: Return or clicking away applies it,
/// Esc keeps the old one. Drag the value sideways to scrub it, as if dragging the thumb (Shift: a tenth
/// as far).
pub fn slider(ui: &mut Ui, spec: &ControlSpec, value: f64, enabled: bool, label_override: Option<&str>) -> SliderOut {
    let t = Tokens::get(ui.ctx());
    let w = ui.available_width();
    // touch: the whole row is the grab zone (a finger's width is more than the track's), the
    // track sits low in it under its label; the value follows the finger's movement rather than
    // jumping to it, and a tap doesn't change it (as in Lightroom's mobile app and iOS's own
    // sliders)
    let touch = crate::is_compact(ui.ctx());
    let (row_h, track_dy) = if touch { (TOUCH_SLIDER_ROW_H, 3.0) } else { (t.slider_row_h, 0.0) };
    let (row, _) = ui.allocate_exact_size(vec2(w, row_h), Sense::hover());
    let (pad_l, pad_r) = side_pad(ui.ctx());
    let label_rect = Rect::from_min_size(pos2(row.left() + pad_l, row.top() + 4.0), vec2(w - pad_l - pad_r, 18.0));
    let track_rect =
        Rect::from_min_max(pos2(row.left() + pad_l, row.top() + 22.0 + track_dy), pos2(row.right() - pad_r, row.top() + 40.0 + track_dy));
    let id = ui.id().with(spec.id);
    let grab = if touch { row } else { track_rect.expand2(vec2(8.0, 2.0)) };
    let resp = ui.interact(grab, id, if enabled { Sense::click_and_drag() } else { Sense::hover() });
    // A click selects the slider for arrow/wheel adjustments. Keep scroll-to-adjust
    // opt-in so hovering an unfocused slider still scrolls the containing panel. (Not on a
    // phone: no keys or wheel there, and a focused widget would hold the keyboard's Esc.)
    // Tab reaching the slider selects it as a click does: the arrow keys then adjust it
    if enabled && !touch && resp.gained_focus() {
        ui.data_mut(|d| d.insert_temp(focused_slider_key(), id));
    }
    if enabled && !touch && (resp.clicked() || resp.drag_started()) {
        resp.request_focus();
        ui.data_mut(|d| d.insert_temp(focused_slider_key(), id));
    }
    // screen readers: a slider named after its control, with its value
    let label_text = crate::i18n::tr(label_override.unwrap_or(spec.label)).to_string();
    resp.widget_info(|| egui::WidgetInfo::slider(enabled, value, label_text.clone()));
    // and its range, step and the value as shown ("+0.50", "5600")
    let shown = shown_value(spec, value);
    ui.ctx().accesskit_node_builder(resp.id, |node| {
        node.set_min_numeric_value(spec.min);
        node.set_max_numeric_value(spec.max);
        node.set_numeric_value_step(spec.step);
        node.set_value(shown);
        if enabled {
            node.add_action(egui::accesskit::Action::Increment);
            node.add_action(egui::accesskit::Action::Decrement);
        }
    });
    // the name: double-click it to reset, as the thumb (a disabled slider's name is only text; and
    // a phone's whole row is the slider's: its label takes no touches of its own)
    let label_resp = ui.interact(label_rect, id.with("label"), if enabled && !touch { Sense::click() } else { Sense::hover() });
    // say how to reset (issue #1436: the double-click was hard to discover), as the curve channels do
    let reset_hint = crate::i18n::tr_format!("{name} — double-click to reset it", name = label_text);
    let label_resp = if enabled { label_resp.on_hover_text(reset_hint.as_str()) } else { label_resp };
    crate::widgets::announce(&label_resp, crate::i18n::tr_format!("Reset {name}", name = label_text.clone()));
    register(ui.ctx(), format!("sliderLabel:{}", spec.id), label_rect);
    register(ui.ctx(), format!("slider:{}", spec.id), track_rect);
    // the value: click it to type an exact one, drag it sideways to scrub (over the label, so it
    // takes the click)
    let value_rect = Rect::from_min_max(pos2(label_rect.right() - 64.0, label_rect.top()), label_rect.max);
    let value_resp = ui.interact(value_rect, id.with("value"), if enabled { Sense::click_and_drag() } else { Sense::hover() });
    crate::widgets::announce(&value_resp, format!("{label_text}: {}", crate::i18n::tr("Click to type a value, drag to change it")));
    register(ui.ctx(), format!("sliderValue:{}", spec.id), value_rect);
    // say that an exact value can be typed here (issue #534: an exact crop angle was hard to find)
    let value_resp = if enabled { value_resp.on_hover_text(crate::i18n::tr("Click to type a value, drag to change it")) } else { value_resp };
    let typing_id = id.with("typing");
    // Keep the short focus-acquisition guard, but let an explicit Return/Escape end the edit.
    let mut typing: Option<(String, u8, bool)> = ui.data(|m| m.get_temp(typing_id));
    // A scrub: the unrounded track position (0..1) it has reached, so a slow drag adds up instead
    // of rounding back to the same step every frame.
    let scrub_id = id.with("scrub-pos");
    let mut scrub: Option<f64> = if value_resp.dragged() || value_resp.drag_stopped() { ui.data(|m| m.get_temp(scrub_id)) } else { None };
    let mut scrub_started = false;
    if enabled
        && scrub.is_none()
        && value_resp.dragged()
        && let (Some(origin), Some(p)) = ui.input(|i| (i.pointer.press_origin(), i.pointer.interact_pos()))
        && (p.x - origin.x).abs() >= SCRUB_START
    {
        // a press on the value that moves sideways scrubs: the edit the press opened is dropped
        typing = None;
        ui.data_mut(|m| m.remove::<(String, u8, bool)>(typing_id));
        ui.memory_mut(|m| m.surrender_focus(crate::text_field::id(&format!("sliderField:{}", spec.id))));
        // the travel so far counts, so the value doesn't lag the pointer by the start distance
        let fine = if ui.input(|i| i.modifiers.shift) { 0.1 } else { 1.0 };
        scrub = Some((spec.position(value) + f64::from((p.x - origin.x) / track_rect.width()) * fine).clamp(0.0, 1.0));
        scrub_started = true;
    }
    if scrub.is_none() {
        ui.data_mut(|m| m.remove::<f64>(scrub_id));
    }
    let value_pressed = value_resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary));
    // Focus on the owned primary press before fast typing; retain the readout's click activation.
    if enabled && typing.is_none() && (value_resp.clicked() || value_pressed) {
        typing = Some((shown_value(spec, value).trim_start_matches('+').to_string(), 0, value_pressed));
    }
    let value_released = ui.input(|i| i.pointer.button_released(egui::PointerButton::Primary));
    // The opening press must complete the readout's original click before the edit continues.
    if value_released && typing.as_ref().is_some_and(|(_, _, pressed)| *pressed) && !value_resp.clicked() {
        typing = None;
        ui.data_mut(|m| m.remove::<(String, u8, bool)>(typing_id));
        ui.memory_mut(|m| m.surrender_focus(crate::text_field::id(&format!("sliderField:{}", spec.id))));
    }
    let mut out = SliderOut::default();
    // value ↔ track position: linear, except for the Kelvin temperature (`ControlSpec::position`)
    let to_x = |v: f64| track_rect.left() + spec.position(v) as f32 * track_rect.width();
    let from_x = |x: f32| spec.value_at(f64::from((x - track_rect.left()) / track_rect.width()));
    // the thumb: a click there doesn't move the value, and resting on it says how to reset (the rest
    // of the track, which is clicked and dragged, gets no tooltip)
    let on_thumb = |p: egui::Pos2| (p.x - to_x(value)).abs() <= THUMB_RING + 1.0;
    if enabled && !resp.dragged() && resp.hover_pos().is_some_and(on_thumb) {
        resp.clone().on_hover_text(reset_hint.as_str());
    }
    let mut v = value;
    if let Some(mut pos) = scrub {
        // scrubbing the value moves it as dragging the thumb would: the pointer's sideways travel
        // over the track's width (a tenth with Shift), on the control's steps
        if !scrub_started && value_resp.dragged() {
            let fine = if ui.input(|i| i.modifiers.shift) { 0.1 } else { 1.0 };
            let dx = ui.input(|i| i.pointer.delta().x);
            pos = (pos + f64::from(dx / track_rect.width()) * fine).clamp(0.0, 1.0);
        }
        let step = spec.step.max(1e-9);
        let nv = ((spec.value_at(pos) / step).round() * step).clamp(spec.min, spec.max);
        if (nv - value).abs() > 1e-12 {
            out.value = Some(nv);
            v = nv;
        }
        out.drag_started = scrub_started;
        if value_resp.drag_stopped() {
            out.drag_stopped = true;
            ui.data_mut(|m| m.remove::<f64>(scrub_id));
        } else {
            ui.data_mut(|m| m.insert_temp(scrub_id, pos));
        }
    } else if let Some((mut text, frames, pressed)) = typing.take() {
        // Keep the owned opening release focused without restarting its draft or selection.
        // egui gives that release's click to the readout, so the field under it gives up the focus
        // whatever frame it lands on (issue #1362): the field takes it back after drawing too.
        let opening_release = pressed && value_released;
        if opening_release {
            ui.memory_mut(|m| m.request_focus(crate::text_field::id(&format!("sliderField:{}", spec.id))));
        }
        let pressed = pressed && !value_released;
        // a child over the value, so the field never moves the rows around it
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(value_rect).layout(egui::Layout::right_to_left(egui::Align::Center)));
        // the old value is selected when the field takes the focus, so typing replaces it
        let field = crate::text_field::TextField::singleline(&format!("sliderField:{}", spec.id), &mut text)
            .font(t.font(12.5))
            .horizontal_align(egui::Align::RIGHT)
            .width(value_rect.width())
            .margin(egui::Margin::ZERO)
            .select_on_focus(true)
            .begin_edit(frames == 0)
            .show(&mut child);
        if (frames < 2 || opening_release) && !matches!(field.ending, Some(crate::text_field::Ending::Return | crate::text_field::Ending::Escape)) {
            field.response.request_focus();
            let frames = frames.saturating_add(1);
            typing = Some((text.clone(), frames, pressed));
            ui.data_mut(|m| m.insert_temp(typing_id, (text, frames, pressed)));
        } else if field.ending.is_some() || !field.editing || !enabled {
            // Esc (or the slider turning off) keeps the old value; Return or clicking away applies
            let cancelled = !enabled || field.cancelled();
            if let Some(nv) = typed_value(spec, &text).filter(|nv| !cancelled && (nv - value).abs() > 1e-12) {
                out.value = Some(nv);
                out.drag_started = true;
                out.drag_stopped = true;
                v = nv;
            }
            ui.data_mut(|m| m.remove::<(String, u8, bool)>(typing_id));
        } else {
            typing = Some((text.clone(), frames, pressed));
            ui.data_mut(|m| m.insert_temp(typing_id, (text, frames, pressed)));
        }
    } else if resp.double_clicked() || label_resp.double_clicked() {
        out.reset = true;
        out.value = Some(spec.default);
        v = spec.default;
        if touch {
            crate::haptics::tap(ui.ctx(), crate::haptics::Haptic::Light);
        }
    } else {
        // touch: a drag that sets off mostly up or down scrolls what holds the slider (the sheet)
        let drag = if touch { touch_drag(ui, &resp, id) } else { None };
        let sliding = match drag {
            Some((mode, was)) => {
                if mode == TouchDrag::Slide && was != Some(TouchDrag::Slide) {
                    out.drag_started = true;
                }
                if mode == TouchDrag::Scroll && resp.dragged() {
                    ui.scroll_with_delta(vec2(0.0, ui.input(|i| i.pointer.delta().y)));
                }
                if resp.drag_stopped() && was == Some(TouchDrag::Slide) {
                    out.drag_stopped = true;
                }
                mode == TouchDrag::Slide
            }
            None => {
                if resp.drag_started() {
                    out.drag_started = true;
                }
                !touch
            }
        };
        if sliding
            && (resp.dragged() || resp.drag_started())
            && let Some(p) = resp.interact_pointer_pos()
        {
            let fine = ui.input(|i| i.modifiers.shift);
            let step = spec.step.max(1e-9);
            let nv = if fine || touch {
                // a tenth of the pointer's movement along the track (a finger moves the thumb from where
                // it is, by all of it); the unrounded position is kept, so a slow drag adds up instead of
                // rounding back to the same step every frame
                let fine_id = id.with("fine-pos");
                let dx = ui.input(|i| i.pointer.delta().x);
                let k = if fine { 0.1 } else { 1.0 };
                let kept = ui.data(|m| m.get_temp::<f64>(fine_id)).filter(|&p| ((spec.value_at(p) / step).round() * step - value).abs() < step / 2.0);
                let pos = (kept.unwrap_or_else(|| spec.position(value)) + f64::from(dx / track_rect.width()) * k).clamp(0.0, 1.0);
                ui.data_mut(|m| m.insert_temp(fine_id, pos));
                spec.value_at(pos)
            } else {
                from_x(p.x)
            };
            let nv = ((nv / step).round() * step).clamp(spec.min, spec.max);
            if (nv - value).abs() > 1e-12 {
                out.value = Some(nv);
                v = nv;
                if touch {
                    detent_haptic(ui.ctx(), spec, value, nv);
                }
            }
        } else if resp.clicked()
            && !touch
            && let Some(p) = resp.interact_pointer_pos()
            // a click on the thumb leaves it there, so the first click of a double-click on it isn't a
            // history step of its own before the reset
            && !on_thumb(p)
        {
            let step = spec.step.max(1e-9);
            let nv = ((from_x(p.x) / step).round() * step).clamp(spec.min, spec.max);
            out.value = Some(nv);
            v = nv;
            out.drag_started = true;
            out.drag_stopped = true;
        }
        if resp.drag_stopped() && !touch {
            out.drag_stopped = true;
        }
        // a screen reader's increment / decrement (VoiceOver: Ctrl-Option-Up / Down)
        let (inc, dec) = ui.input(|i| {
            i.events.iter().fold((false, false), |(inc, dec), e| match e {
                egui::Event::AccessKitActionRequest(r) if r.target_node == resp.id.accesskit_id() => {
                    (inc || r.action == egui::accesskit::Action::Increment, dec || r.action == egui::accesskit::Action::Decrement)
                }
                _ => (inc, dec),
            })
        });
        // Arrow keys work with either a hovered or a focused slider.
        if enabled && out.value.is_none() && typing.is_none() && (ui.rect_contains_pointer(row) || resp.has_focus() || inc || dec) {
            let (up, down, shift) = ui.input_mut(|i| {
                let shift = i.modifiers.shift;
                let m = if shift { egui::Modifiers::SHIFT } else { egui::Modifiers::NONE };
                (i.consume_key(m, egui::Key::ArrowUp), i.consume_key(m, egui::Key::ArrowDown), shift)
            });
            let (up, down) = (up || inc, down || dec);
            if up || down {
                let nv = nudged(spec, value, if up { 1.0 } else { -1.0 } * if shift { 5.0 } else { 1.0 });
                if (nv - value).abs() > 1e-12 {
                    out.value = Some(nv);
                    out.drag_started = true;
                    out.drag_stopped = true;
                    v = nv;
                }
            }
        }
        // Sideways scrolling (a trackpad swipe, a tilting wheel, Shift+wheel) adjusts the slider under
        // the pointer, selected or not: the panel only scrolls vertically, so nothing else wants it.
        // Only a mostly sideways movement counts, so a slightly diagonal swipe that scrolls the panel
        // leaves the sliders it passes alone. Right raises the value, as dragging the thumb right does.
        let side_id = id.with("side-rem");
        let (dx, dy) = ui.input(|i| (i.smooth_scroll_delta.x, i.smooth_scroll_delta.y));
        if !enabled || typing.is_some() || out.value.is_some() || !ui.rect_contains_pointer(row) || dx.abs() <= dy.abs() {
            ui.data_mut(|m| m.remove::<f32>(side_id));
        } else {
            // the vertical part would scroll the panel and move the slider out from under the pointer
            ui.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
            let mut remainder = ui.data(|m| m.get_temp::<f32>(side_id)).unwrap_or(0.0);
            let steps = wheel_nudges(&mut remainder, dx);
            ui.data_mut(|m| m.insert_temp(side_id, remainder));
            let nv = nudged(spec, value, steps);
            if steps != 0.0 && (nv - value).abs() > 1e-12 {
                out.value = Some(nv);
                out.drag_started = true;
                out.drag_stopped = true;
                v = nv;
            }
        }
        // Wheel nudges require an explicitly selected slider, unlike regular panel scrolling.
        // Consume only the vertical component so a trackpad's horizontal scroll still works.
        let wheel_id = id.with("wheel-rem");
        if !resp.has_focus() {
            ui.data_mut(|m| m.remove::<f32>(wheel_id));
        } else if enabled && typing.is_none() && out.value.is_none() && ui.rect_contains_pointer(row) {
            let delta = ui.input_mut(|i| {
                let dy = i.smooth_scroll_delta.y;
                if dy != 0.0 {
                    i.smooth_scroll_delta.y = 0.0;
                }
                dy
            });
            if delta != 0.0 {
                let mut remainder = ui.data(|m| m.get_temp::<f32>(wheel_id)).unwrap_or(0.0);
                let steps = wheel_nudges(&mut remainder, delta);
                ui.data_mut(|m| m.insert_temp(wheel_id, remainder));
                if steps != 0.0 {
                    let nv = nudged(spec, value, steps);
                    if (nv - value).abs() > 1e-12 {
                        out.value = Some(nv);
                        out.drag_started = true;
                        out.drag_stopped = true;
                        v = nv;
                    }
                }
            }
        }
    }
    // paint
    let hovered = resp.hovered() || resp.dragged();
    let text_c = if enabled { t.text_label } else { t.text_disabled };
    let p = ui.painter();
    let font = t.font(if touch { 15.0 } else { 12.5 });
    p.text(label_rect.left_center(), Align2::LEFT_CENTER, crate::i18n::tr(label_override.unwrap_or(spec.label)), font.clone(), text_c);
    if typing.is_none() {
        p.text(label_rect.right_center(), Align2::RIGHT_CENTER, shown_value(spec, v), font, text_c);
        // the scrub pointer, as over Lightroom's values: a drag here changes the value
        if enabled && (value_resp.hovered() || scrub.is_some()) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
    }
    let tx = to_x(v);
    if touch {
        // (the thumb turns into a lens of glass while the finger slides it)
        let held = resp.dragged() && ui.data(|d| d.get_temp::<TouchDrag>(id.with("touch-drag"))) == Some(TouchDrag::Slide);
        let lens = crate::glass::spring(ui.ctx(), id.with("lens"), if held { 1.0 } else { 0.0 }, 0.3, 0.7).max(0.0);
        paint_ios_slider(ui, track_rect, &spec.track, &t, tx, to_x(spec.default), enabled, lens);
        return out;
    }
    if t.filled_sliders {
        paint_filled_slider(ui, track_rect, &spec.track, &t, tx, to_x(spec.default), enabled, hovered);
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        return out;
    }
    let ring = THUMB_RING;
    paint_track(ui, track_rect, &spec.track, &t, tx, ring);
    let p = ui.painter();
    let ring_c = if !enabled {
        t.text_disabled
    } else if hovered {
        t.thumb_hover
    } else {
        t.thumb
    };
    p.circle_filled(pos2(tx, track_rect.center().y), ring, t.chrome);
    p.circle_stroke(pos2(tx, track_rect.center().y), ring, Stroke::new(2.0, ring_c));
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    out
}

/// A desktop slider the Studio way: a 4 pt rounded track, filled in `thumb` from where the
/// control's default is to the handle (an edited slider shows how far it went; a centred one fills
/// outward from its middle), and a solid round handle ringed in the panel's colour, which grows a
/// little under the pointer. Gradient tracks (temperature, hue…) keep their colours and no fill.
fn paint_filled_slider(ui: &Ui, rect: Rect, track: &Track, t: &Tokens, thumb_x: f32, from_x: f32, enabled: bool, hovered: bool) {
    let p = ui.painter();
    let y = rect.center().y;
    const HALF: f32 = 2.0;
    let bar = |a: f32, b: f32| Rect::from_min_max(pos2(a.min(b), y - HALF), pos2(a.max(b), y + HALF));
    if let Some(stops) = track_stops(track) {
        let n = stops.len().max(2) - 1;
        let steps = 48;
        for i in 0..steps {
            let (a, b) = (i as f32 / steps as f32, (i + 1) as f32 / steps as f32);
            let seg = (a * n as f32).floor() as usize;
            let c = lerp(stops[seg.min(n)], stops[(seg + 1).min(n)], a * n as f32 - seg as f32);
            let c = if enabled { c } else { c.gamma_multiply(0.5) };
            // (the end segments carry the track's rounded ends)
            let r = egui::CornerRadius {
                nw: if i == 0 { HALF as u8 } else { 0 },
                sw: if i == 0 { HALF as u8 } else { 0 },
                ne: if i + 1 == steps { HALF as u8 } else { 0 },
                se: if i + 1 == steps { HALF as u8 } else { 0 },
            };
            p.rect_filled(bar(rect.left() + rect.width() * a, rect.left() + rect.width() * b + 0.5), r, c);
        }
    } else {
        p.rect_filled(bar(rect.left(), rect.right()), HALF, t.track);
        if enabled && (thumb_x - from_x).abs() >= 0.5 {
            p.rect_filled(bar(from_x, thumb_x), HALF, t.thumb);
        }
    }
    let center = pos2(thumb_x, y);
    let radius = if hovered && enabled { 7.0 } else { 6.0 };
    let handle = if !enabled {
        t.text_disabled
    } else if hovered {
        t.thumb_hover
    } else {
        t.thumb
    };
    p.circle_filled(center, radius + 2.0, t.chrome);
    p.circle_filled(center, radius, handle);
}

/// A touch slider drawn as iOS 26 draws its own: a 6 pt track, filled with the accent colour from
/// where the control's default is to the thumb (so an edited slider shows how far it went, and a
/// centred one fills outward from its middle), and a white capsule thumb with a soft shadow that
/// swells into a lens of glass while it is held (`lens`, 0–1). Gradient tracks (temperature,
/// hue…) keep their colours.
fn paint_ios_slider(ui: &Ui, rect: Rect, track: &Track, t: &Tokens, thumb_x: f32, from_x: f32, enabled: bool, lens: f32) {
    let p = ui.painter();
    let y = rect.center().y;
    const HALF: f32 = 3.0;
    if let Some(stops) = track_stops(track) {
        let n = stops.len().max(2) - 1;
        let steps = 48;
        for i in 0..steps {
            let (a, b) = (i as f32 / steps as f32, (i + 1) as f32 / steps as f32);
            let seg = (a * n as f32).floor() as usize;
            let local = a * n as f32 - seg as f32;
            let c = lerp(stops[seg.min(n)], stops[(seg + 1).min(n)], local);
            let (x0, x1) = (rect.left() + rect.width() * a, rect.left() + rect.width() * b);
            let r = egui::CornerRadius {
                nw: if i == 0 { 3 } else { 0 },
                sw: if i == 0 { 3 } else { 0 },
                ne: if i + 1 == steps { 3 } else { 0 },
                se: if i + 1 == steps { 3 } else { 0 },
            };
            p.rect_filled(Rect::from_min_max(pos2(x0, y - HALF), pos2(x1 + 0.5, y + HALF)), r, c);
        }
    } else {
        p.rect_filled(Rect::from_min_max(pos2(rect.left(), y - HALF), pos2(rect.right(), y + HALF)), HALF, t.track);
        let (a, b) = (from_x.min(thumb_x), from_x.max(thumb_x));
        if enabled && b - a > 0.5 {
            p.rect_filled(Rect::from_min_max(pos2(a, y - HALF), pos2(b, y + HALF)), HALF, t.accent);
        }
    }
    // (a capsule wider than it is tall; at the ends of the track it stays clear of the panel's edge)
    let size = vec2(38.0, 24.0) * (1.0 + 0.35 * lens);
    let c = pos2(thumb_x.min(rect.right() - 8.0).max(rect.left() + 8.0).min(rect.right()), y);
    let thumb = Rect::from_center_size(c, size);
    let dark = Tokens::is_dark(ui.ctx());
    if lens > 0.01 {
        crate::glass::paint(p, thumb, size.y / 2.0, None, 0.4 * lens);
        // (the glass clears as it swells: what's left of the white thumb fades)
        p.rect_filled(thumb, size.y / 2.0, Color32::from_white_alpha((255.0 * (1.0 - lens).max(0.0)) as u8));
        return;
    }
    p.add(
        egui::epaint::Shadow { offset: [0, 2], blur: 8, spread: 0, color: Color32::from_black_alpha(if dark { 90 } else { 45 }) }
            .as_shape(thumb, size.y / 2.0),
    );
    p.rect_filled(thumb, size.y / 2.0, if enabled { Color32::WHITE } else { t.track });
    if !dark {
        p.rect_stroke(thumb, size.y / 2.0, Stroke::new(0.5, Color32::from_black_alpha(30)), egui::StrokeKind::Outside);
    }
}

/// A tick when a finger's slide lands on the control's default or crosses it (the zero of most
/// sliders, as Lightroom's mobile app does), a light tap on reaching either end of its range.
fn detent_haptic(ctx: &egui::Context, spec: &ControlSpec, from: f64, to: f64) {
    use crate::haptics::{Haptic, tap};
    let half_step = spec.step.max(1e-9) / 2.0;
    let (was_on, on) = ((from - spec.default).abs() < half_step, (to - spec.default).abs() < half_step);
    if on || (!was_on && (from - spec.default).signum() != (to - spec.default).signum()) {
        tap(ctx, Haptic::Selection);
    } else if to <= spec.min || to >= spec.max {
        tap(ctx, Haptic::Light);
    }
}

/// How a finger's drag on a touch slider is taken: undecided for its first few points, then a
/// slide when it went mostly sideways, else a scroll of what holds the slider, as iOS tells them
/// apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TouchDrag {
    Pending,
    Slide,
    Scroll,
}

/// This frame's [`TouchDrag`] for the slider `id` and what it was on the previous frame; `None`
/// when no drag is going on or ending.
fn touch_drag(ui: &Ui, resp: &Response, id: egui::Id) -> Option<(TouchDrag, Option<TouchDrag>)> {
    let key = id.with("touch-drag");
    let was = ui.data(|d| d.get_temp::<TouchDrag>(key));
    if resp.dragged() || resp.drag_started() {
        let mode = match was {
            Some(m) if m != TouchDrag::Pending => m,
            _ => {
                let moved = match (ui.input(|i| i.pointer.press_origin()), resp.interact_pointer_pos()) {
                    (Some(o), Some(p)) => p - o,
                    _ => egui::Vec2::ZERO,
                };
                if moved.length() < 8.0 {
                    TouchDrag::Pending
                } else if moved.x.abs() >= moved.y.abs() {
                    TouchDrag::Slide
                } else {
                    TouchDrag::Scroll
                }
            }
        };
        ui.data_mut(|d| d.insert_temp(key, mode));
        return Some((mode, was));
    }
    if resp.drag_stopped() {
        ui.data_mut(|d| d.remove::<TouchDrag>(key));
        return was.map(|m| (m, was));
    }
    None
}

/// Convert vertical wheel movement to bounded slider steps, keeping sub-notch
/// trackpad movement between frames without amplifying a single large event. One mouse-wheel
/// notch ([`crate::theme::WHEEL_LINE_POINTS`]) is one step.
fn wheel_nudges(remainder: &mut f32, delta: f32) -> f64 {
    const POINTS_PER_STEP: f32 = crate::theme::WHEEL_LINE_POINTS;
    let total = (*remainder + delta).clamp(-10.0 * POINTS_PER_STEP, 10.0 * POINTS_PER_STEP);
    let steps = (total / POINTS_PER_STEP).trunc();
    *remainder = total - steps * POINTS_PER_STEP;
    f64::from(steps)
}

/// `value` moved by `steps` keyboard nudges: one nudge is about 1/200 of the range, a whole
/// number of the control's steps (exposure 0.05, most sliders 1, temperature 50 K).
pub fn nudged(spec: &ControlSpec, value: f64, steps: f64) -> f64 {
    let step = spec.step.max(1e-9);
    let unit = if spec.id == "wb.temp" { 50.0 } else { (((spec.max - spec.min) / 200.0 / step).round().max(1.0)) * step };
    (((value + unit * steps) / step).round() * step).clamp(spec.min, spec.max)
}

/// Collapsible section header ("› Light"). Returns the response (click toggles).
pub fn section_header(ui: &mut Ui, id: &str, title: &str, open: bool, enabled: Option<bool>) -> (Response, Option<bool>) {
    let title = crate::i18n::tr(title);
    let t = Tokens::get(ui.ctx());
    let w = ui.available_width();
    let (r, resp) = ui.allocate_exact_size(vec2(w, 52.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::CollapsingHeader, true, open, title));
    register(ui.ctx(), format!("section:{id}"), r);
    let p = ui.painter();
    if ui.rect_contains_pointer(r) {
        p.rect_filled(r, 0.0, t.chrome.gamma_multiply(1.06));
    }
    let chev = Rect::from_center_size(pos2(r.left() + 30.0, r.center().y), vec2(16.0, 16.0));
    // (it turns down as the body folds open)
    crate::icons::paint_turned(p, chev, Icon::ChevronRight, t.text_label, crate::motion::chevron_turn(ui.ctx(), section_fold_id(id), open));
    p.text(pos2(r.left() + 44.0, r.center().y), Align2::LEFT_CENTER, title, t.semibold(14.0), t.text);
    let mut toggled = None;
    // shown while the pointer is anywhere on the header: `resp.hovered()` turns false as soon as the
    // pointer is over the eye itself (the eye is on top), which hid the eye before it could be
    // clicked (issue #316)
    if let Some(on) = enabled
        && (ui.rect_contains_pointer(r) || !on)
    {
        let eye = Rect::from_center_size(pos2(r.right() - 34.0, r.center().y), vec2(18.0, 18.0));
        let er = ui.interact(eye, ui.id().with(("eye", id)), Sense::click());
        crate::widgets::announce_selected(&er, crate::i18n::tr("Show / hide"), on);
        register(ui.ctx(), format!("sectionEye:{id}"), eye);
        paint(ui.painter(), eye, Icon::Eye, if on { t.icon } else { t.text_disabled });
        if !on {
            ui.painter().line_segment([eye.left_bottom(), eye.right_top()], Stroke::new(1.5, t.text_disabled));
        }
        if er.clicked() {
            toggled = Some(!on);
        }
    }
    (resp, toggled)
}

/// What the body under the [`section_header`] `id` folds under (`motion::fold`).
pub fn section_fold_id(id: &str) -> egui::Id {
    egui::Id::new(("section", id))
}

/// Thin divider between groups.
pub fn divider(ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, t.divider);
}

/// A flyout row inside a section (Curve, Color Mixer, Color Grading…): inset dark box with icon tile.
pub fn flyout_row(ui: &mut Ui, id: &str, title: &str, icon: Icon, open: bool) -> Response {
    let title = crate::i18n::tr(title);
    let t = Tokens::get(ui.ctx());
    let w = ui.available_width();
    let (outer, _) = ui.allocate_exact_size(vec2(w, 40.0), Sense::hover());
    let r = Rect::from_min_max(pos2(outer.left() + 16.0, outer.top() + 4.0), pos2(outer.right() - 16.0, outer.bottom() - 4.0));
    let resp = ui.interact(r, ui.id().with(("flyout", id)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::CollapsingHeader, true, open, title));
    register(ui.ctx(), format!("flyout:{id}"), r);
    let p = ui.painter();
    p.rect_filled(r, 4.0, if resp.hovered() { t.hover.gamma_multiply(0.8) } else { t.inset });
    let tile = Rect::from_min_size(pos2(r.left() + 4.0, r.top() + 4.0), vec2(r.height() - 8.0, r.height() - 8.0));
    p.rect_filled(tile, 3.0, t.chrome.gamma_multiply(1.15));
    paint(p, tile.shrink(3.0), icon, t.text_label);
    p.text(pos2(tile.right() + 10.0, r.center().y), Align2::LEFT_CENTER, title, t.semibold(13.0), t.text_label);
    paint(
        p,
        Rect::from_center_size(pos2(r.right() - 14.0, r.center().y), vec2(16.0, 16.0)),
        if open { Icon::ChevronDown } else { Icon::ChevronLeft },
        t.text_label,
    );
    resp
}

/// The room inside a desktop text box around its text (egui's own 4 × 2 pt felt cramped).
pub const FIELD_MARGIN: egui::Margin = egui::Margin { left: 8, right: 8, top: 5, bottom: 5 };

/// A text box's padding: [`FIELD_MARGIN`] on the desktop; on a phone iOS's 44 pt row, its larger
/// type and the text centred in it.
pub fn text_box<'t>(ui: &Ui, edit: egui::TextEdit<'t>) -> egui::TextEdit<'t> {
    if !crate::is_compact(ui.ctx()) {
        return edit.margin(FIELD_MARGIN);
    }
    edit.font(egui::FontId::proportional(17.0))
        .margin(egui::Margin::symmetric(12, 10))
        .min_size(vec2(0.0, crate::TOUCH_ROW_H))
        .vertical_align(egui::Align::Center)
}

/// A checkbox on the desktop; on a phone an iOS switch at the end of a full-width row, the label
/// first (green when on, its knob sliding across), as iOS forms show an on / off choice.
pub fn check(ui: &mut Ui, on: &mut bool, label: impl Into<egui::WidgetText>) -> Response {
    if !crate::is_compact(ui.ctx()) {
        return ui.checkbox(on, label);
    }
    let t = Tokens::get(ui.ctx());
    let enabled = ui.is_enabled();
    let w = ui.available_width().max(80.0);
    let label: egui::WidgetText = label.into();
    let label = match strip_key_hint(label.text()) {
        plain if plain.len() != label.text().len() => plain.to_string().into(),
        _ => label,
    };
    let galley = label.into_galley(ui, Some(egui::TextWrapMode::Wrap), w - 70.0, egui::TextStyle::Body);
    let (r, mut resp) = ui.allocate_exact_size(vec2(w, (galley.size().y + 16.0).max(44.0)), Sense::click());
    if resp.clicked() && enabled {
        *on = !*on;
        resp.mark_changed();
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, *on, galley.text()));
    // the knob springs across (a little past, and back)
    let k = crate::glass::spring(ui.ctx(), resp.id.with("knob"), if *on { 1.0 } else { 0.0 }, 0.35, 0.68);
    let held = crate::glass::press(ui.ctx(), &resp);
    let p = ui.painter();
    p.galley(pos2(r.left(), r.center().y - galley.size().y / 2.0), galley, if enabled { t.text } else { t.text_disabled });
    // 62 × 28 pt like iOS 26's own, with a capsule knob; its off track is a translucent grey
    let sw = Rect::from_center_size(pos2(r.right() - 31.0, r.center().y), vec2(62.0, 28.0));
    let (off, green) = (t.switch_off, t.switch_on);
    let tone = k.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * tone).round() as u8;
    let track = Color32::from_rgb(mix(off.r(), green.r()), mix(off.g(), green.g()), mix(off.b(), green.b()));
    p.rect_filled(sw, 14.0, if enabled { track } else { track.gamma_multiply(0.5) });
    // held, the knob swells past the track into a lens of glass
    let knob_size = vec2(38.0, 24.0) + vec2(10.0, 12.0) * held;
    let x = sw.left() + 2.0 + knob_size.x / 2.0 - 5.0 * held + (sw.width() - 4.0 - knob_size.x + 10.0 * held) * k;
    let knob = Rect::from_center_size(pos2(x, sw.center().y), knob_size);
    if held > 0.01 {
        crate::glass::paint(p, knob, knob_size.y / 2.0, None, 0.4 * held);
        p.rect_filled(knob, knob_size.y / 2.0, Color32::from_white_alpha((255.0 * (1.0 - held).max(0.0)) as u8));
    } else {
        p.add(egui::epaint::Shadow { offset: [0, 1], blur: 6, spread: 0, color: Color32::from_black_alpha(50) }.as_shape(knob, 12.0));
        p.rect_filled(knob, 12.0, Color32::WHITE);
    }
    resp
}

/// `label` without a trailing keyboard hint such as " (X)" or " (⇧O)": a phone has no keyboard.
pub fn strip_key_hint(label: &str) -> &str {
    match label.rfind(" (") {
        Some(i) if label.ends_with(')') && label[i + 2..label.len() - 1].chars().count() <= 3 => &label[..i],
        _ => label,
    }
}

/// A small bordered text button (Auto, B&W, HDR…); on a phone iOS's borderless, rounded kind, 36 pt
/// tall, the chosen one in the accent colour.
pub fn text_button(ui: &mut Ui, id: &str, label: &str, active: bool) -> Response {
    chip(ui, id, label, active, false)
}

/// One of a row of choices of which exactly one holds (`button:{id}`), drawn as [`text_button`]
/// is, except that the one chosen is also outlined in the label colour: its fill alone stands
/// about 1.4:1 against the others, the outline 3:1 or more in every theme. A screen reader is
/// told it is a radio button.
pub fn choice_button(ui: &mut Ui, id: &str, label: &str, chosen: bool) -> Response {
    chip(ui, id, label, chosen, true)
}

fn chip(ui: &mut Ui, id: &str, label: &str, active: bool, choice: bool) -> Response {
    let label = if id.starts_with("labelSet-") { label } else { crate::i18n::tr(label) };
    let t = Tokens::get(ui.ctx());
    let touch = crate::is_compact(ui.ctx());
    let label = if touch { strip_key_hint(label) } else { label };
    let font = t.semibold(if touch { 13.5 } else { 11.5 });
    // (white on the accent colour, the theme's text colour otherwise)
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, if touch && active { Color32::WHITE } else { t.text });
    let size = if touch { vec2((galley.size().x + 28.0).max(56.0), 36.0) } else { vec2((galley.size().x + 18.0).max(37.0), 24.0) };
    let (r, resp) = ui.allocate_exact_size(size, Sense::click());
    let kind = if choice { egui::WidgetType::RadioButton } else { egui::WidgetType::Button };
    resp.widget_info(|| egui::WidgetInfo::selected(kind, true, active, label));
    register(ui.ctx(), format!("button:{id}"), r);
    let p = ui.painter();
    if touch {
        let fill = if active {
            t.accent
        } else if resp.is_pointer_button_down_on() {
            t.pressed
        } else {
            t.button
        };
        p.rect_filled(r, 9.0, fill);
        p.galley(r.center() - galley.size() / 2.0, galley, t.text);
        return resp;
    }
    let fill = if active {
        t.pressed
    } else if resp.hovered() {
        t.hover
    } else {
        t.button
    };
    let border = if active && choice { t.text_label } else { t.button_border };
    p.rect(r, CornerRadius::same(t.button_radius), fill, Stroke::new(1.0, border), StrokeKind::Inside);
    p.galley(r.center() - galley.size() / 2.0, galley, t.text);
    focus_ring(ui, &t, &resp, r, t.button_radius);
    resp
}

/// The ring a button drawn here shows while it has the keyboard focus (Tab, the arrow keys): 2 pt
/// in the text colour with a 1 pt line of the surface's colour inside it, both within its rect so
/// that a neighbour or the edge of a scrolled list never cuts them. The inner line sets the ring
/// off from the button's fill, and makes it a double line: a chosen [`choice_button`] has a
/// single thin one, so "has the focus" and "is chosen" are told apart by shape, not thickness.
fn focus_ring(ui: &Ui, t: &Tokens, resp: &Response, r: Rect, radius: u8) {
    if resp.has_focus() {
        let p = ui.painter();
        p.rect_stroke(r, CornerRadius::same(radius), Stroke::new(2.0, t.text), StrokeKind::Inside);
        p.rect_stroke(r.shrink(2.0), CornerRadius::same(radius.saturating_sub(2)), Stroke::new(1.0, t.chrome), StrokeKind::Inside);
    }
}

/// How tall a [`tab_button`] is.
const TAB_HEIGHT: f32 = 28.0;

/// A tab of a tab bar (`button:{id}`): its name on the surface it is on, with no border and no
/// fill, so that it does not read as one of the buttons a page holds. The open tab's name is in
/// the text colour over a 2 pt line of that colour along its bottom edge (the bar's own line, when
/// the caller draws one there); the others are in the label colour. The names are a size up from
/// the headings inside a page, which are in the same face. To a screen reader a tab is a button
/// that says whether it is the one chosen (egui has no tab role to give it).
pub fn tab_button(ui: &mut Ui, id: &str, label: &str, open: bool) -> Response {
    let label = crate::i18n::tr(label);
    let t = Tokens::get(ui.ctx());
    let color = if open { t.text } else { t.text_label };
    let galley = ui.painter().layout_no_wrap(label.to_string(), t.semibold(13.5), color);
    let size = vec2((galley.size().x + 20.0).max(37.0), TAB_HEIGHT);
    let (r, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, open, label));
    register(ui.ctx(), format!("button:{id}"), r);
    let p = ui.painter();
    // (on whole pixels, so the line under the open tab is crisp at a whole-number scale)
    let r = egui::emath::GuiRounding::round_to_pixels(r, p.pixels_per_point());
    if resp.hovered() && !open {
        p.rect_filled(r.shrink2(vec2(0.0, 3.0)), CornerRadius::same(4), t.hover);
    }
    let color = if resp.hovered() { t.text } else { color };
    p.galley_with_override_text_color(r.center() - galley.size() / 2.0, galley, color);
    if open {
        p.rect_filled(Rect::from_min_max(pos2(r.left(), r.bottom() - 2.0), r.max), CornerRadius::ZERO, t.text);
    }
    focus_ring(ui, &t, &resp, r, 4);
    resp
}

/// A segmented control: `labels` share the available width equally, `per_row` per row. Each segment is
/// addressable as `button:{id}-{key}` (keys parallel to labels). Returns the clicked index.
pub fn segmented(ui: &mut Ui, id: &str, items: &[(&str, &str)], active: Option<usize>, per_row: usize) -> Option<usize> {
    let t = Tokens::get(ui.ctx());
    let per_row = per_row.max(1);
    if crate::is_compact(ui.ctx()) {
        return ios_segments(ui, id, items.iter().map(|(label, key)| (*label, key.to_string())).collect(), active, per_row);
    }
    let gap = 4.0;
    let w = ui.available_width();
    let seg_w = ((w - gap * (per_row as f32 - 1.0)) / per_row as f32).max(24.0);
    let mut clicked = None;
    for (row_i, row) in items.chunks(per_row).enumerate() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (j, (label, key)) in row.iter().enumerate() {
                let label = crate::i18n::tr(label);
                let i = row_i * per_row + j;
                let (r, resp) = ui.allocate_exact_size(vec2(seg_w, 24.0), Sense::click());
                crate::widgets::announce_selected(&resp, label.to_string(), active == Some(i));
                register(ui.ctx(), format!("button:{id}-{key}"), r);
                let fill = if active == Some(i) {
                    t.pressed
                } else if resp.hovered() {
                    t.hover
                } else {
                    t.button
                };
                let p = ui.painter();
                p.rect(r, CornerRadius::same(t.button_radius), fill, Stroke::new(1.0, t.button_border), StrokeKind::Inside);
                p.text(r.center(), Align2::CENTER_CENTER, label, t.semibold(11.5), t.text);
                if resp.clicked() {
                    clicked = Some(i);
                }
            }
        });
    }
    clicked
}

/// iOS's segmented control (phone dialogs): segments of equal width on a capsule grey track, the
/// chosen one raised on a lighter capsule that slides to the one tapped; more than four wrap to
/// rows of four. Segment `i` is
/// `button:{id}-{i}`. Returns the tapped index.
pub fn ios_segmented(ui: &mut Ui, id: &str, labels: &[&str], active: Option<usize>) -> Option<usize> {
    ios_segments(ui, id, labels.iter().enumerate().map(|(i, l)| (*l, i.to_string())).collect(), active, 4)
}

/// [`ios_segmented`] with `per_row` segments to a row and `button:{id}-{key}` for each segment's
/// `(label, key)`.
fn ios_segments(ui: &mut Ui, id: &str, items: Vec<(&str, String)>, active: Option<usize>, per_row: usize) -> Option<usize> {
    let t = Tokens::get(ui.ctx());
    let w = ui.available_width();
    let mut tapped = None;
    // as few rows as the labels need, filled evenly (five are 3 + 2, not 4 + 1)
    let widest =
        items.iter().map(|(l, _)| ui.painter().layout_no_wrap(crate::i18n::tr(l).to_string(), t.semibold(13.0), t.text).size().x).fold(0.0, f32::max);
    let fit = ((w / (widest + 16.0)).floor() as usize).clamp(1, per_row.max(1));
    let rows = items.len().div_ceil(fit).max(1);
    let per_row = items.len().div_ceil(rows).max(1);
    for (row_i, row) in items.chunks(per_row).enumerate() {
        // iOS 26's: a capsule track, the chosen segment on a raised capsule that slides to the one tapped
        let (track, _) = ui.allocate_exact_size(vec2(w, 36.0), Sense::hover());
        ui.painter().rect_filled(track, 18.0, t.inset);
        let seg_w = track.width() / row.len() as f32;
        let pill_idx = ui.painter().add(egui::Shape::Noop);
        let first = row_i * per_row.max(1);
        let chosen = active.filter(|a| (first..first + row.len()).contains(a)).map(|a| (a - first) as f32);
        if let Some(j) = chosen {
            let id = ui.id().with((id, "pill", row_i));
            let x = crate::glass::spring(ui.ctx(), id, j, 0.4, 0.78);
            let pill = Rect::from_min_size(pos2(track.left() + x * seg_w, track.top()), vec2(seg_w, track.height())).shrink(2.5);
            let dark = Tokens::is_dark(ui.ctx());
            let mut shapes = vec![
                egui::epaint::Shadow { offset: [0, 2], blur: 6, spread: 0, color: Color32::from_black_alpha(if dark { 60 } else { 26 }) }
                    .as_shape(pill, pill.height() / 2.0)
                    .into(),
            ];
            shapes.push(egui::Shape::rect_filled(pill, pill.height() / 2.0, t.segment_pill));
            ui.painter().set(pill_idx, egui::Shape::Vec(shapes));
        }
        for (j, (label, key)) in row.iter().enumerate() {
            let i = first + j;
            let r = Rect::from_min_size(pos2(track.left() + j as f32 * seg_w, track.top()), vec2(seg_w, track.height()));
            let resp = ui.interact(r, ui.id().with((id, i)), Sense::click());
            let on = active == Some(i);
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, on, *label));
            register(ui.ctx(), format!("button:{id}-{key}"), r);
            let p = ui.painter();
            if !on && j > 0 && active != Some(i - 1) {
                p.vline(r.left(), r.shrink2(vec2(0.0, 11.0)).y_range(), Stroke::new(1.0, t.hover));
            }
            let font = if on { t.semibold(13.0) } else { t.font(13.0) };
            p.text(r.center(), Align2::CENTER_CENTER, crate::i18n::tr(label), font, t.text);
            if resp.clicked() {
                tapped = Some(i);
            }
        }
    }
    tapped
}

/// An icon-only button. `active` draws the selected background (tool strip).
pub fn icon_button(ui: &mut Ui, id: &str, icon: Icon, size: egui::Vec2, active: bool, enabled: bool, tooltip: &str) -> Response {
    let tooltip = crate::i18n::tr(tooltip);
    let t = Tokens::get(ui.ctx());
    // a finger needs 40 pt or so
    let size = if crate::is_compact(ui.ctx()) { size.max(vec2(40.0, 40.0)) } else { size };
    let (r, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, active, tooltip));
    register(ui.ctx(), format!("icon:{id}"), r);
    let p = ui.painter();
    let sq = Rect::from_center_size(r.center(), vec2(32.0f32.min(size.x), 32.0f32.min(size.y)));
    let corner = f32::from(t.control_radius) + if t.cards() { 2.0 } else { 0.0 };
    if active {
        p.rect_filled(sq, corner, t.tool_active);
    } else if resp.hovered() && enabled {
        p.rect_filled(sq, corner, t.hover.gamma_multiply(0.7));
    }
    let c = if !enabled {
        t.text_disabled
    } else if active {
        t.pick
    } else if resp.hovered() {
        t.text
    } else {
        t.icon
    };
    let glyph = (size.x.min(size.y) * 0.62).min(22.0);
    paint(p, Rect::from_center_size(r.center(), vec2(glyph, glyph)), icon, c);
    if !tooltip.is_empty() { resp.on_hover_text(tooltip) } else { resp }
}

/// Star rating row (5 stars); returns a new rating when clicked (clicking the current rating clears).
pub fn stars(ui: &mut Ui, id: &str, rating: u8, size: f32) -> Option<u8> {
    let t = Tokens::get(ui.ctx());
    let mut out = None;
    let mut hover_n = None;
    let mut rects = [Rect::NOTHING; 5];
    for (i, slot) in (0..5u8).zip(rects.iter_mut()) {
        let (r, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
        crate::widgets::announce_selected(&resp, format!("{} ★", i + 1), i < rating);
        register(ui.ctx(), format!("star:{id}:{}", i + 1), r);
        if resp.hovered() {
            hover_n = Some(i + 1);
        }
        if resp.clicked() {
            out = Some(if rating == i + 1 { 0 } else { i + 1 });
        }
        *slot = r;
    }
    // Hover previews the rating a click would set: stars 1..=hovered light up. The hovered star also gets the
    // icon_button hover square, so hovering the current rating (where the preview looks the same) still shows feedback.
    for (i, r) in (0..5u8).zip(rects) {
        if hover_n == Some(i + 1) {
            ui.painter().rect_filled(r, 4.0, t.hover.gamma_multiply(0.7));
        }
        let filled = star_lit(i, rating, hover_n);
        paint(ui.painter(), r.shrink(size * 0.12), if filled { Icon::StarFilled } else { Icon::Star }, if filled { t.star } else { t.icon });
    }
    out
}

/// Whether the star at zero-based index `i` is drawn filled: the hovered star's rating previews over the current one.
fn star_lit(i: u8, rating: u8, hover: Option<u8>) -> bool {
    i < hover.unwrap_or(rating)
}

/// A borderless dropdown button: `text` followed by a painted chevron (no font glyph needed).
pub fn dropdown(ui: &mut Ui, id: &str, text: &str, font: egui::FontId, color: Color32) -> Response {
    dropdown_within(ui, id, text, font, color, f32::INFINITY)
}

/// [`dropdown`] at most `max` wide, chevron included: a longer `text` is cut short with …, and
/// shown in full on hover.
pub fn dropdown_within(ui: &mut Ui, id: &str, text: &str, font: egui::FontId, color: Color32, max: f32) -> Response {
    let t = Tokens::get(ui.ctx());
    let galley = one_line(ui.painter(), text, font, color, max - 20.0);
    let elided = galley.elided;
    let size = vec2(galley.size().x + 20.0, galley.size().y.max(if crate::is_compact(ui.ctx()) { 40.0 } else { 20.0 }));
    let (r, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, text));
    register(ui.ctx(), format!("dropdown:{id}"), r);
    let c = if resp.hovered() { t.text } else { color };
    ui.painter().galley(pos2(r.left(), r.center().y - galley.size().y / 2.0), galley, c);
    paint(ui.painter(), Rect::from_center_size(pos2(r.right() - 10.0, r.center().y), vec2(16.0, 16.0)), Icon::ChevronDown, c);
    if elided { resp.on_hover_text(text) } else { resp }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_efficiency_nef_explains_the_codec_limit_and_workarounds() {
        let reason = "Nikon High Efficiency NEF (HE / HE★, a JPEG XS-based codec with no public specification; shoot RAW Lossless compressed to edit the raw data)";
        let explanation = preview_only_explanation(reason);
        assert!(explanation.contains("TicoRAW"), "{explanation}");
        assert!(explanation.contains("does not currently have a license"), "{explanation}");
        assert!(!explanation.contains("cannot decode"), "{explanation}");
        assert!(explanation.contains("Lossless compressed NEF"), "{explanation}");
        assert!(explanation.contains("DNG"), "{explanation}");
        assert!(!explanation.contains("yet"), "{explanation}");
        assert!(preview_only_explanation("unsupported CR3").contains("unsupported CR3"));
    }

    /// Issue #647: hovering a star previews the rating a click would set.
    #[test]
    fn hovered_star_previews_its_rating() {
        let lit = |rating, hover| (0..5u8).filter(|&i| star_lit(i, rating, hover)).count();
        assert_eq!(lit(2, None), 2, "no hover shows the rating");
        assert_eq!(lit(2, Some(4)), 4);
        assert_eq!(lit(5, Some(1)), 1);
        assert_eq!(lit(0, Some(3)), 3);
    }

    #[test]
    fn keyboard_hints_are_stripped_but_other_parentheses_stay() {
        assert_eq!(strip_key_hint("Swap Aspect (X)"), "Swap Aspect");
        assert_eq!(strip_key_hint("Flip Overlay (⇧O)"), "Flip Overlay");
        assert_eq!(strip_key_hint("Delete (⌫)"), "Delete");
        assert_eq!(strip_key_hint("Resize (long edge)"), "Resize (long edge)");
        assert_eq!(strip_key_hint("Exposure"), "Exposure");
        assert_eq!(strip_key_hint("(X)"), "(X)", "a label that is only a hint keeps it");
    }

    #[test]
    fn nudges_are_a_sensible_step() {
        let spec = |id, min, max, step| ControlSpec {
            id,
            label: "",
            section: lightcraft_develop::controls::Section::Light,
            min,
            max,
            default: 0.0,
            step,
            decimals: 2,
            track: Track::Plain,
        };
        assert!((nudged(&spec("light.exposure", -5.0, 5.0, 0.01), 0.0, 1.0) - 0.05).abs() < 1e-9);
        assert_eq!(nudged(&spec("light.contrast", -100.0, 100.0, 1.0), 10.0, -5.0), 5.0);
        assert_eq!(nudged(&spec("wb.temp", 2000.0, 50000.0, 1.0), 6500.0, 1.0), 6550.0);
        assert_eq!(nudged(&spec("light.contrast", -100.0, 100.0, 1.0), 99.0, 5.0), 100.0, "clamped");
    }

    #[test]
    fn wheel_steps_accumulate_small_movements_and_preserve_direction() {
        // one mouse-wheel notch is one step, whatever a notch scrolls
        let notch = crate::theme::WHEEL_LINE_POINTS;
        let mut remainder = 0.0;
        assert_eq!(wheel_nudges(&mut remainder, notch * 0.375), 0.0);
        assert_eq!(wheel_nudges(&mut remainder, notch * 0.625), 1.0);
        assert_eq!(remainder, 0.0);
        assert_eq!(wheel_nudges(&mut remainder, -notch * 2.5), -2.0);
        assert_eq!(wheel_nudges(&mut remainder, -notch * 0.5), -1.0);
        assert_eq!(remainder, 0.0);
        assert_eq!(wheel_nudges(&mut remainder, notch), 1.0, "a single notch is a single nudge");
        assert_eq!(remainder, 0.0);
        assert_eq!(wheel_nudges(&mut remainder, notch * 50.0), 10.0, "a single huge event is capped");
    }

    /// Shift-drag moves a tenth of the pointer's way along the track, also on the Kelvin track (where a pixel of
    /// fine drag near the centre is about 4 K): a pointer creeping a pixel a frame still adds up to steps.
    #[test]
    fn a_slow_shift_drag_adds_up() {
        let ctx = egui::Context::default();
        let temp = lightcraft_develop::controls::find("wb.temp").unwrap();
        let mut value = 7000.0;
        let frame = |time: f64, events: Vec<egui::Event>, value: f64| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(320.0, 120.0))),
                time: Some(time),
                events,
                ..Default::default()
            };
            let mut out = SliderOut::default();
            ctx.run_ui(input, |ui| out = slider(ui, temp, value, true, None)).textures_delta.clear();
            let track = take_registry(&ctx).into_iter().find(|(id, _)| id == "slider:wb.temp").unwrap().1;
            (out, track)
        };
        // Shift is held from the start (the modifiers stay until they change)
        let (_, track) = frame(0.0, vec![egui::Event::ModifiersChanged(egui::Modifiers::SHIFT)], value);
        let mut at = pos2(track.left() + temp.position(value) as f32 * track.width(), track.center().y);
        let press =
            |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::SHIFT };
        frame(0.1, vec![egui::Event::PointerMoved(at), press(at, true)], value);
        let moves = 60;
        for i in 0..moves {
            at.x += 1.0;
            let (out, _) = frame(0.2 + f64::from(i) * 0.02, vec![egui::Event::PointerMoved(at)], value);
            value = out.value.unwrap_or(value);
        }
        frame(2.0, vec![press(at, false)], value);
        // the drag starts once the pointer has moved a few pixels; every pixel after that counts
        let lo = temp.value_at(temp.position(7000.0) + f64::from(moves - 10) / f64::from(track.width()) * 0.1);
        let hi = temp.value_at(temp.position(7000.0) + f64::from(moves) / f64::from(track.width()) * 0.1);
        assert!(value >= lo - 50.0 && value <= hi + 50.0 && value > 7000.0, "{value} K, expected {lo:.0}–{hi:.0} K");
    }

    /// Typed values (issue #322): signs, a decimal comma, units; on the control's steps, in range.
    #[test]
    fn typed_values_are_stepped_and_clamped() {
        let exposure = lightcraft_develop::controls::find("light.exposure").unwrap();
        let contrast = lightcraft_develop::controls::find("light.contrast").unwrap();
        let temp = lightcraft_develop::controls::find("wb.temp").unwrap();
        assert_eq!(typed_value(exposure, "1.5"), Some(1.5));
        assert_eq!(typed_value(exposure, " +0,25 "), Some(0.25));
        assert_eq!(typed_value(exposure, "-12"), Some(exposure.min), "clamped");
        assert_eq!(typed_value(contrast, "33.4"), Some(33.0), "on the control's steps");
        assert_eq!(typed_value(contrast, "-20 %"), Some(-20.0));
        assert_eq!(typed_value(temp, "5600 K"), Some(5600.0));
        for bad in ["", "abc", "1.2.3", "NaN", "inf", "--1"] {
            assert_eq!(typed_value(contrast, bad), None, "{bad:?}");
        }
    }
}

#[cfg(test)]
mod access_tests {
    use super::*;

    /// Screen readers see the custom widgets: sliders by control name with their value, buttons
    /// by label or tooltip.
    #[test]
    fn custom_widgets_describe_themselves() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        ctx.enable_accesskit();
        let spec = lightcraft_develop::controls::find("light.exposure").unwrap();
        let mut found = Vec::new();
        let mut ranges = Vec::new();
        for _ in 0..3 {
            let out = ctx.run_ui(egui::RawInput::default(), |ui| {
                let _ = slider(ui, spec, 0.5, true, None);
                let _ = text_button(ui, "auto", "Auto", false);
                let _ = icon_button(ui, "trash", Icon::Trash, vec2(24.0, 24.0), false, true, "Delete mask");
            });
            let mut out = out;
            out.textures_delta.clear();
            if let Some(update) = out.platform_output.accesskit_update.take() {
                ranges = update
                    .nodes
                    .iter()
                    .filter(|(_, n)| n.role() == egui::accesskit::Role::Slider)
                    .map(|(_, n)| (n.min_numeric_value(), n.max_numeric_value()))
                    .collect();
                found = update
                    .nodes
                    .iter()
                    .map(|(_, n)| (format!("{:?}", n.role()), n.label().unwrap_or_default().to_string(), n.numeric_value()))
                    .collect();
            }
        }
        let has = |role: &str, label: &str| found.iter().any(|(r, l, _)| r == role && l == label);
        assert!(has("Slider", "Exposure"), "{found:?}");
        assert!(found.iter().any(|(r, l, v)| r == "Slider" && l == "Exposure" && *v == Some(0.5)), "the slider's value: {found:?}");
        assert!(ranges.contains(&(Some(spec.min), Some(spec.max))), "the slider's range: {ranges:?}");
        assert!(has("Button", "Auto"), "{found:?}");
        assert!(has("Button", "Delete mask"), "{found:?}");
    }
}

/// Names a widget for screen readers (`name` replaces what egui gave it): for a plain egui field
/// whose caption is a separate label beside it.
pub(crate) fn screen_reader_name(ctx: &egui::Context, id: egui::Id, name: &str) {
    ctx.accesskit_node_builder(id, |node| node.set_label(name.to_string()));
}

/// A hand-drawn clickable area (an icon, a tile, a row) announced as a button named `name`.
pub(crate) fn announce(resp: &egui::Response, name: impl Into<String>) {
    let name = name.into();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, resp.enabled(), name.clone()));
}

/// The same for one that is on or off (a toggle, a selected row or swatch).
pub(crate) fn announce_selected(resp: &egui::Response, name: impl Into<String>, selected: bool) {
    let name = name.into();
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, resp.enabled(), selected, name.clone()));
}

/// `r`, named `name` for screen readers: a plain egui field or slider whose caption is drawn
/// beside it, or whose hint is all it shows.
pub(crate) fn named(r: egui::Response, name: &str) -> egui::Response {
    screen_reader_name(&r.ctx, r.id, name);
    r
}

/// `text` laid out on one line, cut short with … when it is wider than `max` (the galley's
/// `elided` says whether it was): for text painted into a fixed box (a tile, a row before its
/// buttons), which a plain `Painter::text` would draw past.
pub(crate) fn one_line(painter: &egui::Painter, text: &str, font: egui::FontId, color: Color32, max: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max.max(1.0));
    painter.layout_job(job)
}

/// `text` shortened to fit `max` (as `width` measures it): leading folders drop first
/// (`Users/me/Pictures/Lightroom` → `…/Pictures/Lightroom`) so the end of a path, which says the
/// most, stays; a single name still too long loses its end (`2024-06-12 Tri…`: folders tend to
/// differ at the start). Cuts fall on characters. The result is never empty.
pub(crate) fn elide_head(text: &str, max: f32, width: impl Fn(&str) -> f32) -> String {
    if width(text) <= max {
        return text.to_string();
    }
    let mut rest = text;
    while let Some(i) = rest.find('/') {
        rest = rest.get(i + 1..).unwrap_or("");
        let cand = format!("…/{rest}");
        if width(&cand) <= max {
            return cand;
        }
    }
    let n = rest.chars().count();
    for keep in (1..n).rev() {
        let cand: String = rest.chars().take(keep).chain(std::iter::once('…')).collect();
        if width(&cand) <= max {
            return cand;
        }
    }
    "…".to_string()
}

#[cfg(test)]
#[path = "widgets/tests_numeric_entry.rs"]
mod numeric_entry_tests;

#[cfg(test)]
#[path = "widgets/tests_slider_reset.rs"]
mod slider_reset_tests;

fn focused_slider_key() -> egui::Id {
    egui::Id::new("lightcraft-focused-slider")
}

/// Whether the widget with keyboard focus is a develop slider a click selected. It owns only the
/// arrow keys (and the wheel): every other shortcut still works, as in Lightroom after a drag.
pub fn slider_has_focus(ctx: &egui::Context) -> bool {
    let focused = ctx.memory(|m| m.focused());
    focused.is_some() && focused == ctx.data(|d| d.get_temp::<egui::Id>(focused_slider_key()))
}
