import LightCraftCore
import SwiftUI

/// What `crop.expandModels` says: the models that can paint the new area, and the Generate running.
nonisolated struct ExpandModels: Decodable, Equatable {
    nonisolated struct Model: Decodable, Equatable, Identifiable {
        var id: String
        var label: String
        var note: String?
        var available: Bool?
        var ready: Bool?
        var disabled: Bool?
        var downloadBytes: Double?
        var licence: String?
        var credit: String?
        var `default`: Bool?
    }

    var models: [Model]
    var running: Running?

    nonisolated struct Running: Decodable, Equatable {
        var photo: UInt64?
        var model: String?
    }

    /// The models this device can use (now, or once downloaded).
    var offered: [Model] { models.filter { $0.available != false } }

    /// The one Generate uses: the one chosen if it's offered, else the default.
    func chosen(_ pick: String) -> Model? {
        offered.first { $0.id == pick } ?? offered.first { $0.default == true } ?? offered.first
    }
}

/// Crop ▸ Expand, as Lightroom's Generative Expand: the crop may reach past the photo's edges (the
/// Expand tile, in the aspect tiles), and Generate paints the new area on this device with the
/// model in Fill With (MI-GAN, a download, or Classic, texture from the photo). Generate runs in the
/// background (`crop.generate {wait: false}`): its progress and Cancel are in its button, and the
/// host stores the fill (one undo step) and says so when it ends.
struct ExpandControls: View {
    @Environment(AppModel.self) private var model
    @State private var info: ExpandModels?
    @State private var offer: ExpandModels.Model?

    var body: some View {
        let running = model.activity.first { $0.kind == "expand" }
        let chosen = info?.chosen(model.prefs.expandModel)
        VStack(spacing: 6) {
            HStack(spacing: 8) {
                if let running {
                    Button {
                        model.perform("activity.cancel", ["id": .int(Int64(clamping: running.id))])
                    } label: {
                        Label {
                            HStack(spacing: 0) {
                                Text("Generating")
                                Text(verbatim: running.total > 0 ? "… \(Int(Double(running.done) / Double(running.total) * 100))%" : "…")
                                    .monospacedDigit()
                            }
                        } icon: {
                            ProgressView()
                        }
                    }
                    .buttonStyle(.glass)
                    .accessibilityHint("Stops painting the new area")
                } else {
                    Button("Generate", systemImage: "wand.and.stars") { generate(chosen) }
                        .buttonStyle(.glassProminent)
                        .disabled(info == nil)
                }
                fillWith(chosen)
            }
            .font(.subheadline.weight(.semibold))
            Text(status(chosen))
                .font(.caption)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, 16)
        }
        .task(id: Key(revision: model.settledRevision, running: running != nil, downloading: downloading)) {
            // again while a model downloads, until it's here
            repeat {
                guard let engine = model.engine else { return }
                info = try? await engine.execute("crop.expandModels", [:], as: ExpandModels.self)
                guard downloading else { return }
                try? await Task.sleep(for: .milliseconds(600))
            } while !Task.isCancelled
        }
        .alert(offer.map { String(localized: "Download \($0.label)?") } ?? "", isPresented: Binding(get: { offer != nil }, set: { if !$0 { offer = nil } })) {
            Button("Download") {
                if let m = offer { model.perform("models.download", ["id": .string(m.id), "acknowledged": true]) }
                offer = nil
            }
            Button("Cancel", role: .cancel) { offer = nil }
        } message: {
            if let m = offer {
                Text("Uses the \(m.label) model, a \(Int((m.downloadBytes ?? 0) / 1e6)) MB download (\(m.licence ?? "") licence). \(m.credit ?? "").")
            }
        }
    }

    private nonisolated struct Key: Hashable {
        let revision: UInt64
        let running: Bool
        let downloading: Bool
    }

    /// A model download under way (the activity stack's row).
    private var downloading: Bool {
        model.activity.contains { $0.kind == "download" }
    }

    /// Fill With: the models this device can use; one still to download asks first.
    private func fillWith(_ chosen: ExpandModels.Model?) -> some View {
        Menu {
            Section("Fill With") {
                ForEach(info?.offered ?? []) { m in
                    Button {
                        if m.ready == true {
                            model.prefs.expandModel = m.id
                        } else if m.disabled != true {
                            offer = m
                        }
                    } label: {
                        if m.id == chosen?.id {
                            Label(m.label, systemImage: "checkmark")
                        } else {
                            Text(m.label)
                        }
                        Text(describe(m))
                    }
                    .disabled(m.disabled == true)
                }
            }
        } label: {
            Label(chosen?.label ?? String(localized: "Fill With"), systemImage: "paintbrush.pointed")
        }
        .buttonStyle(.glass)
        .accessibilityLabel("Fill With")
        .accessibilityValue(chosen?.label ?? "")
    }

    /// A model's note: what it is, and what's needed first.
    private func describe(_ m: ExpandModels.Model) -> String {
        let note = m.note ?? ""
        if m.disabled == true { return "\(note) · \(String(localized: "Turned off in Settings → AI Models"))" }
        if m.ready != true, let bytes = m.downloadBytes { return String(localized: "\(note) · \(Int(bytes / 1e6)) MB download") }
        return note
    }

    /// What the new area holds, and what to do.
    private func status(_ chosen: ExpandModels.Model?) -> String {
        if let by = model.editor.setting("expand_fill", "model").string {
            let label = info?.models.first { $0.id == by }?.label ?? by
            return String(localized: "The new area was painted by \(label) on this device.")
        }
        return String(localized: "The checkerboard shows area with nothing in it yet: Generate fills it.")
    }

    private func generate(_ chosen: ExpandModels.Model?) {
        guard let chosen else { return }
        guard chosen.ready == true else {
            if chosen.disabled != true { offer = chosen }
            return
        }
        Task {
            if await model.run("crop.generate", ["model": .string(chosen.id), "wait": false]) != nil {
                model.buzz(.light)
            }
        }
    }
}

/// The area a photo doesn't cover yet (the crop's Expand), as image editors show transparency.
struct Checkerboard: View {
    var body: some View {
        Canvas { ctx, size in
            let side: CGFloat = 8
            ctx.fill(Path(CGRect(origin: .zero, size: size)), with: .color(Color(white: 0.32)))
            var dark = Path()
            for row in 0...Int(size.height / side) {
                for col in 0...Int(size.width / side) where (row + col) % 2 == 0 {
                    dark.addRect(CGRect(x: CGFloat(col) * side, y: CGFloat(row) * side, width: side, height: side))
                }
            }
            ctx.fill(dark, with: .color(Color(white: 0.22)))
        }
        .accessibilityHidden(true)
    }
}
