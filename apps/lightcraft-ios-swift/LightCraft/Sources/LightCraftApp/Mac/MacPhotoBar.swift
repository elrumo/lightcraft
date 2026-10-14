#if os(macOS)
import LightCraftCore
import SwiftUI

/// The bar under the loupe, as Lightroom lays it out: how the photo is shown (the grid, the loupe,
/// Compare, Survey), its rating and flag with copying its edits in the middle, and the zoom and
/// Before / After at the right. A narrow window drops the view modes, then the words.
struct MacPhotoBar: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let id = model.state.active
        let ids = id.map { [$0] } ?? []
        let summary = id.flatMap { model.library.index(of: $0) }.flatMap { model.library.loaded(at: $0) }
        // (one bar, drawn for the width it has: ViewThatFits would build each variant's AppKit menus and
        // buttons to measure them, and a window that can't settle them aborts)
        GeometryReader { geo in
            bar(summary, ids, views: geo.size.width >= 600, words: geo.size.width >= 740)
                .padding(.horizontal, 12)
                .frame(width: geo.size.width, height: geo.size.height)
        }
        .frame(height: 38)
        .background(.bar)
        .overlay(alignment: .top) { Divider() }
        .disabled(id == nil)
    }

    private func bar(_ summary: PhotoSummary?, _ ids: [UInt64], views: Bool, words: Bool) -> some View {
        HStack(spacing: 16) {
            if views { viewModes }
            Spacer(minLength: 8)
            stars(summary?.rating ?? 0, ids)
            flags(summary, ids)
            copy(ids, words: words)
            Spacer(minLength: 8)
            zoom
            beforeAfter
        }
    }

    // MARK: view modes

    /// Grid (back), Loupe (here), Compare and Survey (of the photos chosen, two or more).
    private var viewModes: some View {
        let chosen = Array(model.library.selected)
        return HStack(spacing: 2) {
            barButton("square.grid.3x3", "Grid") { model.path = [] }
            barButton("rectangle.center.inset.filled", "Loupe", on: true) {}
            barButton("rectangle.split.2x1", "Compare") { model.path.append(.compare(chosen)) }
                .disabled(chosen.count < 2)
            barButton("square.grid.2x2", "Survey") { model.path.append(.survey(chosen)) }
                .disabled(chosen.count < 2)
        }
    }

    // MARK: rating and flag

    /// A click on a star rates the photo by it; on the star it has, takes the rating away.
    private func stars(_ rating: Int, _ ids: [UInt64]) -> some View {
        HStack(spacing: 1) {
            ForEach(1...5, id: \.self) { n in
                Button {
                    model.perform("photo.rate", ["ids": AppModel.ids(ids), "rating": .int(Int64(n == rating ? 0 : n))])
                } label: {
                    Image(systemName: n <= rating ? "star.fill" : "star")
                        .font(.system(size: 13))
                        .foregroundStyle(n <= rating ? Color.yellow : .secondary)
                        .frame(width: 20, height: 24)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("\(n) stars")
                .accessibilityAddTraits(n == rating ? .isSelected : [])
            }
        }
    }

    /// Pick and Reject; a click on the one it has unflags it.
    private func flags(_ summary: PhotoSummary?, _ ids: [UInt64]) -> some View {
        let set = { (flag: String) in
            model.perform("photo.flag", ["ids": AppModel.ids(ids), "flag": .string(flag)])
        }
        return HStack(spacing: 2) {
            barButton("flag.fill", "Pick", on: summary?.isPick == true) { set(summary?.isPick == true ? "none" : "pick") }
            barButton("flag.slash.fill", "Reject", on: summary?.isReject == true, tint: summary?.isReject == true ? .red : nil) {
                set(summary?.isReject == true ? "none" : "reject")
            }
        }
    }

    // MARK: copy

    /// Copy Edit Settings, and the gear: paste them, choose which to copy or paste, start over.
    private func copy(_ ids: [UInt64], words: Bool) -> some View {
        HStack(spacing: 4) {
            Button {
                Task { if await model.run("develop.copy") != nil { model.show(String(localized: "Edit settings copied")) } }
            } label: {
                if words {
                    Label("Copy Edit Settings", systemImage: "doc.on.doc")
                } else {
                    Image(systemName: "doc.on.doc")
                }
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
            .help(Text("Copy Edit Settings"))
            Menu {
                Button("Paste Edit Settings", systemImage: "doc.on.clipboard") { model.perform("develop.paste") }
                    .disabled(model.library.state?.clipboard != true)
                Button("Copy Settings…", systemImage: "checklist") { model.sheet = .copySettings }
                Button("Paste Selected Settings…", systemImage: "checklist") { model.sheet = .pasteSettings(ids) }
                    .disabled(model.library.state?.clipboard != true)
                Divider()
                Button("Reset All Edits", systemImage: "arrow.counterclockwise") { model.perform("develop.reset") }
            } label: {
                Image(systemName: "gearshape")
            }
            .menuStyle(.borderlessButton)
            .fixedSize()
        }
    }

    // MARK: zoom and Before / After

    /// "Fit" or the percent of the photo's own pixels, and what the menu asks the loupe for.
    private var zoom: some View {
        let editor = model.editor
        return Menu {
            Button("Fit") { editor.zoomAsk = .fit }
            Button("Fill") { editor.zoomAsk = .fill }
            Divider()
            ForEach([1.0, 2.0, 4.0], id: \.self) { x in
                Button("\(Int(x * 100)) %") { editor.zoomAsk = .actual(x) }
            }
        } label: {
            Text(editor.zoomedIn ? "\(editor.zoomPercent ?? 100) %" : "Fit")
                .font(.callout.monospacedDigit())
                .frame(minWidth: 44)
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
        .help(Text("Zoom"))
    }

    private var beforeAfter: some View {
        Toggle(isOn: Bindable(model.editor).beforeAfter) {
            Image(systemName: "rectangle.split.2x1")
        }
        .toggleStyle(.button)
        .controlSize(.small)
        .help(Text("Before / After"))
    }

    // MARK: pieces

    private func barButton(_ symbol: String, _ title: LocalizedStringKey, on: Bool = false, tint: Color? = nil, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: 13))
                .frame(width: 26, height: 24)
                .background(on ? Color.primary.opacity(0.18) : .clear, in: .rect(cornerRadius: 6))
                .foregroundStyle(tint ?? (on ? Color.primary : Color.secondary))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help(Text(title))
        .accessibilityLabel(Text(title))
        .accessibilityAddTraits(on ? .isSelected : [])
    }
}
#endif
