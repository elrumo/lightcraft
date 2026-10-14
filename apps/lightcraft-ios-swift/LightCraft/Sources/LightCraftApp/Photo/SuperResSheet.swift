import LightCraftCore
import SwiftUI

/// Super Resolution… (the photo's "…" menu), as the egui app's Enhance dialog: until the model is
/// here, what it is, its size, licence and credit, and Download (nothing downloads before that);
/// while it downloads, its progress; then the photo's size before and after, and Enlarge, which
/// runs in the background (its row in the activity stack, ✕ stops it) and adds the result next to
/// the original.
struct SuperResSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var status: JSONValue = .null
    @State private var error: String?

    private static let modelID = "nomos-span-2x"

    var body: some View {
        let info = status["models"].array?.first { $0["id"].string == Self.modelID } ?? .null
        let installed = info["installed"].bool == true
        let download = status["download"]["status"]
        let downloading = status["download"]["id"].string == Self.modelID && download["running"].bool == true
        NavigationStack {
            List {
                if status["disabled"].bool == true {
                    Section {
                        Text("Super Resolution is turned off: turn its model on in Settings → AI Models.")
                    }
                } else if installed {
                    photoSection
                    Section {
                        Text("The photo is enlarged to twice its width and height with an AI model, with your edits applied, and added next to the original (a 16-bit sRGB TIFF), stacked with it. The original isn't changed. This takes a while on large photos.")
                            .font(.footnote)
                    } footer: {
                        credit(info)
                    }
                } else {
                    Section {
                        Text("Super Resolution enlarges a photo to twice its width and height with an AI model. The model isn't part of LightCraft, and everything else works without it.")
                        if let bytes = info["sizeBytes"].double {
                            LabeledContent("Download", value: String(format: "%.1f MB", bytes / 1e6))
                        }
                    } footer: {
                        credit(info)
                    }
                    if downloading {
                        Section {
                            let done = download["done"].double ?? 0, total = max(download["total"].double ?? 0, 1)
                            ProgressView(value: done / total) {
                                Text(String(format: "%.2f / %.2f MB", done / 1e6, total / 1e6)).font(.footnote).monospacedDigit()
                            }
                            Button("Cancel Download", role: .destructive) { model.perform("enhance.model.cancel") }
                        }
                    }
                    if let e = error ?? download["error"].string {
                        Section {
                            Text("The download didn't work: \(e)").foregroundStyle(.red).font(.footnote)
                        }
                    }
                }
            }
            .navigationTitle("Super Resolution")
            .inlineTitle()
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(installed || downloading ? "Close" : "Not Now") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    if installed {
                        Button("Enlarge") { enlarge() }
                            .disabled(status["photo"].isNull || status["photo"]["tooLarge"].bool == true || status["disabled"].bool == true)
                    } else if !downloading {
                        Button(error != nil || download["error"].string != nil ? "Try Again" : "Download") { fetch() }
                            .disabled(status["disabled"].bool == true)
                    }
                }
            }
        }
        .presentationDetents([.medium, .large])
        .task {
            // while the sheet is open: the model's state and the download's progress
            while !Task.isCancelled {
                if let engine = model.engine, let s = try? await engine.execute("enhance.model.status") {
                    status = s
                }
                try? await Task.sleep(for: .milliseconds(400))
            }
        }
    }

    @ViewBuilder private var photoSection: some View {
        let p = status["photo"]
        if p.isNull {
            Section { Text("Select a photo to enlarge") }
        } else {
            Section {
                LabeledContent(p["name"].string ?? "", value: "")
                LabeledContent("Size") {
                    Text(verbatim: "\(p["width"].int ?? 0) × \(p["height"].int ?? 0)  →  \(p["outWidth"].int ?? 0) × \(p["outHeight"].int ?? 0)  (\(megapixels(p)))")
                        .monospacedDigit()
                }
                if p["tooLarge"].bool == true {
                    Text("This is too large for Super Resolution here. Crop the photo first: the limit is \(Int((p["limitPixels"].double ?? 0) / 1e6)) megapixels.")
                        .foregroundStyle(.red).font(.footnote)
                }
            }
        }
    }

    private func megapixels(_ p: JSONValue) -> String {
        let mp = (p["outWidth"].double ?? 0) * (p["outHeight"].double ?? 0) / 1e6
        return mp < 1 ? "< 1 MP" : String(format: "%.0f MP", mp)
    }

    private func credit(_ info: JSONValue) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            if let credit = info["credit"].string, let licence = info["licence"].string {
                Text(verbatim: "\(credit) · \(licence)")
            }
            HStack(spacing: 16) {
                if let url = info["licenceUrl"].string.flatMap(URL.init(string:)) { Link("Licence", destination: url) }
                if let url = info["homeUrl"].string.flatMap(URL.init(string:)) { Link("About the model", destination: url) }
            }
        }
    }

    /// Download the model: the user has pressed Download.
    private func fetch() {
        error = nil
        Task {
            guard let engine = model.engine else { return }
            do {
                _ = try await engine.execute("enhance.model.download", ["id": .string(Self.modelID), "acknowledged": true])
            } catch {
                self.error = error.localizedDescription
            }
        }
    }

    /// Enlarge in the background; the sheet goes, the activity stack shows the progress.
    private func enlarge() {
        let model = model
        dismiss()
        Task {
            guard let engine = model.engine else { return }
            do {
                _ = try await engine.execute("enhance.superRes")
                model.show(String(localized: "Super Resolution added"))
            } catch {
                let text = error.localizedDescription
                model.show(text.contains("cancelled") ? String(localized: "Super Resolution cancelled") : String(localized: "Super Resolution failed: \(text)"),
                           error: !text.contains("cancelled"))
            }
        }
    }
}
