#if os(iOS)
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
            if #available(iOS 27.0, *) {
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
@available(iOS 27.0, *)
private struct TapToSegmentRow: View {
    @State private var ready: Bool?
    @State private var progress: Double?
    @State private var asking = false
    @State private var failure: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Apple tap-to-segment model").font(.headline)
            Text("Selects any object from taps, including parts you add or leave out.").font(.subheadline).foregroundStyle(.secondary)
            if let progress {
                ProgressView(value: progress)
            } else if ready == true {
                Label("On this device. Managed by iOS.", systemImage: "checkmark.circle").font(.caption).foregroundStyle(.secondary)
            } else if ready == false {
                Button("Download…") { asking = true }
            }
            if let failure { Text(failure).font(.caption).foregroundStyle(.red) }
        }
        .task { ready = await VisionIterativeProvider.isReady() }
        .alert("Download Apple's selection model?", isPresented: $asking) {
            Button("Download") { start() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("iOS downloads it from Apple the first time and manages it. It is Apple's model, under Apple's terms.")
        }
    }

    private func start() {
        progress = 0
        failure = nil
        Task {
            do {
                try await VisionIterativeProvider.download { value in
                    Task { @MainActor in progress = value }
                }
                ready = await VisionIterativeProvider.isReady()
            } catch {
                failure = error.localizedDescription
            }
            progress = nil
        }
    }
}
#endif
