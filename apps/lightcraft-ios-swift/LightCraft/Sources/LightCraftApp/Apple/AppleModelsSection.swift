import LightCraftCore
import SwiftUI

/// Settings ▸ AI Models, the top: what selects objects. Apple's models are built into iOS and run on
/// this device; Classic is LightCraft's own guess (no model). The models the engine lists (MI-GAN,
/// Super Resolution…) follow below, as before.
struct AppleModelsSection: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var prefs = model.prefs
        Section {
            Picker("Selection", selection: $prefs.selectionEngine) {
                Text("Automatic").tag("auto")
                Text("Apple").tag("apple")
                Text("Classic").tag("classic")
            }
            VStack(alignment: .leading, spacing: 4) {
                Text("Apple Vision").font(.headline)
                Text("Lifts the subject and selects objects from your taps, on this device. Built into iOS.")
                    .font(.subheadline).foregroundStyle(.secondary)
            }
            if #available(iOS 27.0, macOS 27.0, *) {
                TapToSegmentRow()
            } else {
                Text("Picking any object by tapping needs iOS 27. On this version Apple Vision selects the subject and whatever stands out in front.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        } header: {
            Text("Selection")
        } footer: {
            Text("Automatic uses Apple's models where this device has them. Classic is LightCraft's own guess and needs no model.")
        }
    }
}

/// Apple's tap-to-segment model: iOS downloads it on request and keeps it.
@available(iOS 27.0, macOS 27.0, *)
private struct TapToSegmentRow: View {
    @Environment(AppModel.self) private var model
    @State private var task: Task<Void, Never>?
    @State private var ready: Bool?
    @State private var progress: Double?
    @State private var asking = false
    @State private var failure: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Apple tap-to-segment model").font(.headline)
            Text("Selects any object from taps, including parts you add or leave out.").font(.subheadline).foregroundStyle(.secondary)
            if let progress {
                ProgressView(value: progress).transition(.opacity)
            } else if ready == true {
                Label("On this device. Managed by iOS.", systemImage: "checkmark.circle").font(.caption).foregroundStyle(.secondary).transition(.opacity)
            } else if ready == false {
                Button("Download…") { asking = true }.transition(.opacity)
            }
            if let failure { Text(failure).font(.caption).foregroundStyle(.red).transition(.opacity.combined(with: .move(edge: .top))) }
        }
        .animation(.snappy, value: progress == nil)
        .animation(.snappy, value: ready)
        .animation(.snappy, value: failure)
        .task { ready = await VisionIterativeProvider.isReady() }
        .alert("Download Apple's selection model?", isPresented: $asking) {
            Button("Download") { start() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("iOS downloads it from Apple the first time and manages it. It is Apple's model, under Apple's terms.")
        }
    }

    /// The download runs in the activity stack like every other (`activity.external`): its ✕ stops it.
    private func start() {
        progress = 0
        failure = nil
        task = Task {
            let row = await model.run("activity.external", ["action": "start", "label": "Downloading Apple's selection model", "cancellable": true])?["id"].int
            do {
                try await VisionIterativeProvider.download { value in
                    Task { @MainActor in
                        progress = value
                        guard let row else { return }
                        model.perform("activity.external", ["action": "progress", "id": .int(row), "done": .int(Int64(value * 100))])
                        if model.activity.first(where: { $0.id == UInt64(row) })?.cancelling == true { task?.cancel() }
                    }
                }
                ready = await VisionIterativeProvider.isReady()
                if ready != true {
                    failure = await VisionIterativeProvider.problem() ?? String(localized: "iOS did not finish downloading the model. Try again later.")
                }
            } catch is CancellationError {
            } catch {
                failure = error.localizedDescription
            }
            if let row { await model.run("activity.external", ["action": "end", "id": .int(row)]) }
            progress = nil
            task = nil
        }
    }
}
