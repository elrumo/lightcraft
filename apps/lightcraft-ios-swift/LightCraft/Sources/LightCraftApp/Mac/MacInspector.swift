#if os(macOS)
import LightCraftCore
import SwiftUI

/// The editor's right-hand side as Lightroom lays it out: the open tool's panel, and along the
/// window's edge a column of tool icons (Presets, Edit, Crop, Remove, Red Eye, Masking; History,
/// Versions; Keywords and Info at the bottom). The panel is always there while a photo is open.
struct MacInspector: View {
    var body: some View {
        // The panel gets the width the column has, whatever its controls would like: a column that
        // widens itself when a box opens is worse than a control that shrinks.
        GeometryReader { geo in
            HStack(spacing: 0) {
                MacToolPanel()
                    .frame(width: max(geo.size.width - MacToolStrip.width - 1, 0))
                Divider()
                MacToolStrip()
            }
        }
    }
}

/// The open tool's controls.
private struct MacToolPanel: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let tool = model.editor.tool ?? .edit
        Group {
            switch tool {
            case .edit:
                MacEditPanel()
            case .presets:
                MacPanel(title: "Presets") { PresetsPanel() }
            case .info:
                MacPanel(title: "Info") { InfoPanel() }
            case .crop:
                MacCropPanel()
            case .masking:
                MacMaskingPanel()
            case .remove:
                MacRemovePanel()
            case .redEye:
                MacRedEyePanel()
            }
        }
        .environment(\.toolColumn, true)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

/// The column of tools along the window's edge.
private struct MacToolStrip: View {
    @Environment(AppModel.self) private var model

    private static let tools: [Tool] = [.presets, .edit, .crop, .remove, .redEye, .masking]
    static let width: CGFloat = 50

    var body: some View {
        VStack(spacing: 4) {
            ForEach(Self.tools) { tool in toolButton(tool) }
            Divider().frame(width: 22).padding(.vertical, 6)
            button("History", symbol: "clock.arrow.circlepath") { model.sheet = .history(versions: false) }
            button("Versions", symbol: "square.stack") { model.sheet = .history(versions: true) }
            Spacer(minLength: 0)
            button("Keywords", symbol: "tag") { model.sheet = .keywords }
            toolButton(.info)
        }
        .padding(.vertical, 12)
        .padding(.horizontal, 6)
        .frame(width: Self.width)
        .disabled(model.state.active == nil)
    }

    private func toolButton(_ tool: Tool) -> some View {
        let editor = model.editor
        let on = (editor.tool ?? .edit) == tool
        return button(tool.label, symbol: tool.symbol, on: on) {
            withAnimation(.snappy) { editor.tool = tool }
            if tool.hasScreen { model.perform("develop.cancelInteraction") }
        }
    }

    private func button(_ title: String, symbol: String, on: Bool = false, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: 15))
                .frame(width: 34, height: 34)
                .background(on ? Color.accentColor.opacity(0.28) : .clear, in: .rect(cornerRadius: 8))
                .foregroundStyle(on ? Color.accentColor : .primary)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help(Text(LocalizedStringKey(title)))
        .accessibilityLabel(Text(LocalizedStringKey(title)))
        .accessibilityAddTraits(on ? .isSelected : [])
    }
}
#endif
