import LightCraftCore
import SwiftUI

/// Photo Merge: HDR, Panorama or HDR Panorama of the chosen photos, with its options and a preview
/// drawn as they change (`host.merge.preview`); Merge runs in the background, in the activity
/// stack, and adds the result to the library (`host.merge.start`).
struct MergeSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let command: String
    let ids: [UInt64]
    @State private var options = MergeOptions()
    @State private var info: JSONValue = .null
    @State private var error: String?
    @State private var drawing = false

    private var hdr: Bool { command != "merge.panorama" }
    private var pano: Bool { command != "merge.hdr" }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    ZStack {
                        Color.lcTertiaryFill
                        if let p = model.images.pictures["merge"] {
                            Image(decorative: p.image, scale: 1).resizable().scaledToFit()
                        }
                        if drawing { ProgressView() }
                    }
                    .frame(height: 220)
                    .listRowInsets(EdgeInsets())
                    if let error {
                        Label(error, systemImage: "exclamationmark.triangle").foregroundStyle(.orange)
                    }
                } footer: {
                    Text("\(ids.count) photos")
                }
                if hdr {
                    Section("HDR") {
                        Toggle("Auto Align", isOn: $options.align)
                        Picker("Deghost", selection: $options.deghost) {
                            Text("None").tag("none")
                            Text("Low").tag("low")
                            Text("Medium").tag("medium")
                            Text("High").tag("high")
                        }
                        if options.deghost != "none" {
                            Toggle("Show Deghost Overlay", isOn: $options.showOverlay)
                        }
                    }
                }
                if pano {
                    Section("Panorama") {
                        Picker("Projection", selection: $options.projection) {
                            Text("Auto Select").tag("auto")
                            Text("Spherical").tag("spherical")
                            Text("Cylindrical").tag("cylindrical")
                            Text("Perspective").tag("perspective")
                        }
                        VStack(alignment: .leading) {
                            LabeledContent("Boundary Warp", value: Int(options.boundaryWarp).formatted())
                            Slider(value: $options.boundaryWarp, in: 0...100, step: 1)
                        }
                        Toggle("Fill Edges", isOn: $options.fillEdges)
                        Toggle("Auto Crop", isOn: $options.autoCrop)
                        if hdr {
                            Stepper(value: $options.bracket, in: 0...9) {
                                LabeledContent("Photos per Bracket", value: options.bracket == 0 ? String(localized: "Auto") : options.bracket.formatted())
                            }
                        }
                    }
                }
                Section {
                    Toggle("Auto Settings", isOn: $options.autoSettings)
                    Toggle("Create Stack", isOn: $options.stack)
                }
            }
            .navigationTitle(LocalizedStringKey(Self.title(command)))
            .inlineTitle()
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel", systemImage: "xmark") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Merge") {
                        Task { if await model.request("host.merge.start", params) != nil { dismiss() } }
                    }
                }
            }
            // closed: the preview being drawn is dropped
            .onDisappear { Task { await model.request("host.merge.cancel") } }
            // a change of option draws a new preview (the one being drawn is dropped)
            .task(id: options) {
                guard let engine = model.engine else { return }
                drawing = true
                defer { drawing = false }
                do {
                    info = try await engine.request("host.merge.preview", params)
                    error = nil
                } catch let e as ErrorBody where e.code == "cancelled" {
                    // (a newer preview took its place)
                    _ = e
                } catch {
                    self.error = error.localizedDescription
                }
            }
        }
    }

    private var params: JSONValue {
        ["command": .string(command), "ids": AppModel.ids(ids), "options": options.json]
    }

    static func title(_ command: String) -> String {
        switch command {
        case "merge.panorama": "Panorama Merge Preview"
        case "merge.hdrPanorama": "HDR Panorama Merge Preview"
        default: "HDR Merge Preview"
        }
    }
}

/// The merge dialog's options, as the `merge.*` commands take them.
nonisolated struct MergeOptions: Hashable {
    var align = true
    var deghost = "none"
    var showOverlay = false
    var autoSettings = true
    var stack = false
    var projection = "auto"
    var boundaryWarp: Double = 0
    var autoCrop = true
    var fillEdges = false
    var bracket = 0

    var json: JSONValue {
        ["align": .bool(align), "deghost": .string(deghost), "showOverlay": .bool(showOverlay), "autoSettings": .bool(autoSettings),
         "stack": .bool(stack), "projection": .string(projection), "boundaryWarp": .double(boundaryWarp), "autoCrop": .bool(autoCrop),
         "fillEdges": .bool(fillEdges), "bracket": .int(Int64(bracket))]
    }
}
