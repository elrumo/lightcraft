import LightCraftCore
import SwiftUI

// What a Mac's sheets need that a phone's pages don't: a size of their own, grouped forms where a
// phone has tables, the keys drawn as keys, and an empty state that is not a line of grey text.
// Everything here is the same call on both platforms; the iPhone and iPad keep what they had.

/// A list of sections: a table on a phone, a grouped form (System Settings') on a Mac, where the rows
/// sit in rounded groups and a label and its control share a row.
struct LCList<Content: View>: View {
    @ViewBuilder var content: () -> Content

    var body: some View {
        #if os(macOS)
        Form { content() }
        #else
        List { content() }
        #endif
    }
}

/// An empty list's explanation: an icon, what is missing and what to do about it, centred in the
/// group instead of a line of grey text at its top.
struct LCEmpty: View {
    let title: LocalizedStringKey
    let symbol: String
    var message: LocalizedStringKey?

    var body: some View {
        ContentUnavailableView {
            Label(title, systemImage: symbol)
        } description: {
            if let message { Text(message) }
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 12)
        #if os(macOS)
        .listRowBackground(Color.clear)
        .listRowInsets(EdgeInsets())
        #endif
    }
}

/// A sheet's header: what it is for, drawn large (a tinted icon, its name, a line of explanation),
/// above the groups it holds.
struct LCHero: View {
    let symbol: String
    let title: Text
    var subtitle: Text?
    var tint: Color = .accentColor

    var body: some View {
        VStack(spacing: 6) {
            Image(systemName: symbol)
                .font(.system(size: 30, weight: .regular))
                .foregroundStyle(.white)
                .frame(width: 60, height: 60)
                .background(tint.gradient, in: .rect(cornerRadius: 14))
                .padding(.bottom, 4)
            title.font(.title3.weight(.semibold)).multilineTextAlignment(.center)
            if let subtitle {
                subtitle.font(.callout).foregroundStyle(.secondary).multilineTextAlignment(.center)
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 8)
        #if os(macOS)
        .listRowBackground(Color.clear)
        #endif
    }
}

/// A row that is a button: the whole row takes the click, and it lights under the pointer (a plain
/// button in a grouped form is a small push button off to one side).
struct LCRowButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        Row(configuration: configuration)
    }

    private struct Row: View {
        let configuration: ButtonStyleConfiguration
        @State private var hovering = false

        var body: some View {
            configuration.label
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
                .opacity(configuration.isPressed ? 0.6 : 1)
                #if os(macOS)
                .background(hovering ? Color.primary.opacity(0.07) : .clear, in: .rect(cornerRadius: 6))
                .onHover { hovering = $0 }
                #endif
        }
    }
}

extension View {
    /// A button in a list of rows is a row on a Mac (`LCRowButtonStyle`); a phone's list already makes it one.
    @ViewBuilder func lcRowButton() -> some View {
        #if os(macOS)
        buttonStyle(LCRowButtonStyle())
        #else
        self
        #endif
    }
}

/// A shortcut as the keyboard draws it: a cap for each key (⇧ ⌘ E).
struct KeyCaps: View {
    let shortcut: String

    var body: some View {
        HStack(spacing: 4) {
            ForEach(Array((KeyChord(shortcut: shortcut)?.caps ?? [shortcut]).enumerated()), id: \.offset) { _, cap in
                Text(verbatim: cap)
                    .font(.system(.callout, design: .rounded).weight(.medium))
                    .frame(minWidth: 18)
                    .padding(.horizontal, 5)
                    .padding(.vertical, 2)
                    .background(.quaternary.opacity(0.6), in: .rect(cornerRadius: 6))
                    .overlay { RoundedRectangle(cornerRadius: 6).strokeBorder(.quaternary) }
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(verbatim: ShortcutsSheet.symbols(shortcut)))
    }
}

#if os(macOS)
extension Sheet {
    /// How big a Mac shows it, in points: a dialog's few fields, a form of settings, a page of rows.
    /// (A sheet is a narrow form by default, whatever its content asks for, so every sheet is sized
    /// here, fitted to this.)
    var macSize: CGSize {
        switch self {
        case .newAlbum, .renameAlbum, .newSmartAlbumFromView: CGSize(width: 440, height: 220)
        case .autoStack: CGSize(width: 480, height: 300)
        case .captureTime: CGSize(width: 500, height: 420)
        case .cull: CGSize(width: 500, height: 380)
        case .superRes: CGSize(width: 500, height: 480)
        case .contactSheet, .shareLink, .syncStatus, .syncChoice: CGSize(width: 500, height: 440)
        case .labelNames: CGSize(width: 500, height: 580)
        case .createPreset, .copySettings, .pasteSettings: CGSize(width: 500, height: 620)
        case .about: CGSize(width: 480, height: 620)
        case .filter, .editSidebar, .metadataPresets, .activity, .history, .keywords: CGSize(width: 520, height: 600)
        case .export: CGSize(width: 560, height: 680)
        case .smartAlbum: CGSize(width: 580, height: 560)
        case .keywordSets: CGSize(width: 560, height: 680)
        case .albums: CGSize(width: 520, height: 620)
        case .cameraImport, .lightroom: CGSize(width: 620, height: 560)
        case .shortcuts: CGSize(width: 620, height: 680)
        case .allMetadata: CGSize(width: 680, height: 640)
        case .renamePhotos: CGSize(width: 640, height: 680)
        case .merge: CGSize(width: 620, height: 700)
        case .importReview: CGSize(width: 720, height: 640)
        case .settings, .shareFiles: CGSize(width: 480, height: 360)
        }
    }
}

extension View {
    /// The size a Mac gives a sheet: this wide, this tall to start with (a long form scrolls).
    func macSheetFrame(_ size: CGSize) -> some View {
        frame(minWidth: size.width, idealWidth: size.width, maxWidth: size.width, minHeight: size.height, idealHeight: size.height)
    }
}
#endif
