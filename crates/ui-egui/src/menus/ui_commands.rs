use super::*;

pub use lightcraft_apphost::ui_commands::UI_COMMANDS;

/// The "Edit → Language" entries, one per language in [`crate::i18n::Locale::ALL`], so a language
/// has a matching menu command (also listed through the control channel). Tests check coverage.
pub const LANGUAGE_COMMANDS: &[UiCommand] = &[
    ("app.language.english", crate::i18n::Locale::En.name(), None, "Edit>Language"),
    // The picker order (`Locale::for_picker`): alphabetical by the language's own name after
    // English; Cyrillic, Georgian, then CJK names follow the Latin ones. A test keeps them equal.
    ("app.language.indonesian", crate::i18n::Locale::Id.name(), None, "Edit>Language"),
    ("app.language.catalan", crate::i18n::Locale::Ca.name(), None, "Edit>Language"),
    ("app.language.czech", crate::i18n::Locale::Cs.name(), None, "Edit>Language"),
    ("app.language.german", crate::i18n::Locale::De.name(), None, "Edit>Language"),
    ("app.language.spanish", crate::i18n::Locale::Es.name(), None, "Edit>Language"),
    ("app.language.french", crate::i18n::Locale::Fr.name(), None, "Edit>Language"),
    ("app.language.italian", crate::i18n::Locale::It.name(), None, "Edit>Language"),
    ("app.language.polish", crate::i18n::Locale::Pl.name(), None, "Edit>Language"),
    ("app.language.portuguese", crate::i18n::Locale::PtBr.name(), None, "Edit>Language"),
    ("app.language.finnish", crate::i18n::Locale::Fi.name(), None, "Edit>Language"),
    ("app.language.swedish", crate::i18n::Locale::Sv.name(), None, "Edit>Language"),
    ("app.language.vietnamese", crate::i18n::Locale::Vi.name(), None, "Edit>Language"),
    ("app.language.russian", crate::i18n::Locale::Ru.name(), None, "Edit>Language"),
    ("app.language.ukrainian", crate::i18n::Locale::Uk.name(), None, "Edit>Language"),
    ("app.language.georgian", crate::i18n::Locale::Ka.name(), None, "Edit>Language"),
    ("app.language.japanese", crate::i18n::Locale::Ja.name(), None, "Edit>Language"),
    ("app.language.simplifiedChinese", crate::i18n::Locale::ZhHans.name(), None, "Edit>Language"),
    ("app.language.traditionalChinese", crate::i18n::Locale::ZhHant.name(), None, "Edit>Language"),
];

/// Every UI command: the languages, then everything else. `xtask parity` reads both tables from
/// this file, so an id listed in `docs/parity-checklist.md` is checked wherever it is declared.
pub fn ui_commands() -> impl Iterator<Item = &'static UiCommand> {
    LANGUAGE_COMMANDS.iter().chain(UI_COMMANDS)
}

/// The language a Language-menu command selects, if the id is one. The engine and the UI both go
/// through here, so the menu, the settings row and the control channel agree on the mapping.
pub fn language_from_command(id: &str) -> Option<crate::i18n::Locale> {
    match id {
        "app.language.english" => Some(crate::i18n::Locale::En),
        "app.language.simplifiedChinese" => Some(crate::i18n::Locale::ZhHans),
        "app.language.traditionalChinese" => Some(crate::i18n::Locale::ZhHant),
        "app.language.japanese" => Some(crate::i18n::Locale::Ja),
        "app.language.portuguese" => Some(crate::i18n::Locale::PtBr),
        "app.language.french" => Some(crate::i18n::Locale::Fr),
        "app.language.spanish" => Some(crate::i18n::Locale::Es),
        "app.language.german" => Some(crate::i18n::Locale::De),
        "app.language.russian" => Some(crate::i18n::Locale::Ru),
        "app.language.ukrainian" => Some(crate::i18n::Locale::Uk),
        "app.language.polish" => Some(crate::i18n::Locale::Pl),
        "app.language.czech" => Some(crate::i18n::Locale::Cs),
        "app.language.italian" => Some(crate::i18n::Locale::It),
        "app.language.georgian" => Some(crate::i18n::Locale::Ka),
        "app.language.catalan" => Some(crate::i18n::Locale::Ca),
        "app.language.swedish" => Some(crate::i18n::Locale::Sv),
        "app.language.finnish" => Some(crate::i18n::Locale::Fi),
        "app.language.indonesian" => Some(crate::i18n::Locale::Id),
        "app.language.vietnamese" => Some(crate::i18n::Locale::Vi),
        _ => None,
    }
}
