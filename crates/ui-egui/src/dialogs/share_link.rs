//! The body of `Dialog::ShareLink` (an album's Share Link…): when the link ends, whether people can download the
//! originals or add photos, and an optional password, before `album.share` makes it (`docs/sync.md` → *Share an
//! album*).

use crate::theme::Tokens;

/// The dialog's contents; true when Return in the password field confirms it.
pub(super) fn body(
    ui: &mut egui::Ui,
    t: &Tokens,
    name: &str,
    days: &mut u32,
    originals: &mut bool,
    upload: &mut bool,
    password: &mut String,
) -> bool {
    ui.label(egui::RichText::new(name).strong().color(t.text));
    ui.label(
        egui::RichText::new(crate::i18n::tr(
            "Anyone with the link can see this album in a browser, without an account: its photos by day, as you have edited them, one by one or as a slideshow.",
        ))
        .color(t.text_dim),
    );
    let ends: [(u32, &str); 5] = [(0, "Never"), (1, "1 Day"), (7, "1 Week"), (30, "1 Month"), (365, "1 Year")];
    super::choices(ui, "Link Ends", "shareEnds", &ends, days);
    crate::panels::settings::check(ui, "shareOriginals", originals, "People can download the originals");
    crate::panels::settings::check(ui, "shareUpload", upload, "People can add photos to this album");
    if *upload {
        ui.label(egui::RichText::new(crate::i18n::tr("Photos people add become yours: in this album, on all your devices.")).color(t.text_dim));
    }
    ui.label(egui::RichText::new(crate::i18n::tr("Password (optional)")).color(t.text_label));
    let r = crate::text_field::TextField::singleline("field:sharePassword", password)
        .password(true)
        .hint(crate::i18n::tr("No password"))
        .width(f32::INFINITY)
        .show(ui);
    let short = too_short(password);
    let note = if short { "A password has at least 4 characters." } else { "Visitors type it before they see anything." };
    ui.label(egui::RichText::new(crate::i18n::tr(note)).color(if short { t.caution } else { t.text_dim }));
    r.ending == Some(crate::text_field::Ending::Return) && !short
}

/// A password that was typed but is shorter than the server's shortest ([`lightcraft_engine::cloud_sync::SHARE_PASSWORD_MIN`]).
pub(super) fn too_short(password: &str) -> bool {
    !password.is_empty() && password.chars().count() < lightcraft_engine::cloud_sync::SHARE_PASSWORD_MIN
}
