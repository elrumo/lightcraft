import LightCraftCore
import SwiftUI
import UniformTypeIdentifiers

/// Settings ▸ Faces, as the egui app's tab: recognition on or off (once a model is chosen), how
/// far the scan is, the face detector and the recognition models (download with their terms
/// first, use, remove), and a model file of one's own (`faces.models.*`, `faces.enable`).
struct FacesSettings: View {
    @Environment(AppModel.self) private var model
    @State private var list: JSONValue = .null
    @State private var downloads: [JSONValue] = []
    @State private var offer: FaceModelOffer?
    @State private var importing = false
    @State private var peak = 0
    @State private var tick = 0

    var body: some View {
        let models = list["models"].array ?? []
        let on = list["enabled"].bool == true
        let runtime = list["runtime"].bool == true
        // a chosen recognition model that is installed: without one the switch has nothing to run
        let usable = models.contains { $0["id"] == list["embedder"] && $0["installed"].bool == true && !list["embedder"].isNull }
        List {
            if list.isNull {
                ProgressView()
            } else if list["dir"].isNull {
                Text("This build has nowhere to keep face models.")
            } else {
                Section {
                    Toggle("Recognise Faces (Experimental)", isOn: Binding(get: { on }, set: { new in
                        Task {
                            await model.run("faces.enable", ["enabled": .bool(new)])
                            reload()
                        }
                    }))
                    .disabled(!on && !(runtime && usable))
                    if on, usable, let scan = model.state.faces {
                        if scan.pendingPhotos == 0 {
                            Text("\(scan.indexedFaces) faces learned.").foregroundStyle(.secondary)
                        } else {
                            ProgressView(value: Double(max(peak - scan.pendingPhotos, 0)), total: Double(max(peak, 1))) {
                                Text("Scanning photos for faces: \(scan.pendingPhotos) left · \(scan.indexedFaces) faces so far").font(.footnote)
                            }
                        }
                    }
                } footer: {
                    if !runtime {
                        Text("This build cannot run recognition models: they can be added and chosen, not used.")
                    } else if !usable {
                        Text("Choose a model below (Use) to start.")
                    } else {
                        Text("Suggests who is in a photo from the faces you have named. Everything stays on this device.")
                    }
                }
                Section {
                    ForEach(models.filter { $0["role"] == "detector" }, id: \.self) { m in row(m, runtime: runtime) }
                } header: {
                    Text("Face Detector")
                } footer: {
                    if !models.contains(where: { $0["role"] == "detector" && $0["installed"].bool == true }) {
                        Text("Detect Faces needs it: a small one-time download.")
                    }
                }
                Section("Recognition Models") {
                    ForEach(models.filter { $0["role"] == "embedder" }, id: \.self) { m in row(m, runtime: runtime) }
                    Button("Add a Model File…", systemImage: "doc.badge.plus") { importing = true }
                }
                let errors = (list["catalog"]["errors"].array ?? []).compactMap(\.string).prefix(3)
                if !errors.isEmpty {
                    Section {
                        ForEach(Array(errors), id: \.self) { Text("catalog.json: \($0)").foregroundStyle(.orange).font(.footnote) }
                    }
                }
            }
        }
        .navigationTitle("Faces")
        .inlineTitle()
        .onChange(of: model.state.faces?.pendingPhotos) { _, left in peak = max(peak, left ?? 0) }
        .task(id: tick) {
            // the list, and again while a download runs
            while !Task.isCancelled {
                guard let engine = model.engine else { return }
                list = (try? await engine.execute("faces.models.list")) ?? list
                downloads = (try? await engine.execute("faces.models.downloads"))?["downloads"].array ?? []
                peak = max(peak, model.state.faces?.pendingPhotos ?? 0)
                guard downloads.contains(where: { $0["state"] == "running" || $0["state"] == "done" }) else { return }
                try? await Task.sleep(for: .milliseconds(500))
            }
        }
        .fileImporter(isPresented: $importing, allowedContentTypes: [UTType(filenameExtension: "onnx") ?? .data]) { result in
            guard case .success(let url) = result else { return }
            Task { await inspect(url) }
        }
        .sheet(item: $offer, onDismiss: reload) { FaceModelSheet(offer: $0) }
    }

    private func reload() {
        tick += 1
    }

    /// A model: its name, size and state, its licence, how fast it is (and whether it passed its
    /// test), its download; Use, Remove, Download (its terms first), Cancel.
    private func row(_ m: JSONValue, runtime: Bool) -> some View {
        let id = m["id"].string ?? ""
        let installed = m["installed"].bool == true, selected = m["selected"].bool == true
        let dl = installed ? nil : downloads.first { $0["id"].string == id }
        let state = dl?["state"].string
        return VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(m["name"].string ?? id)
                if let size = m["sizeBytes"].double { Text(Self.mb(size)).foregroundStyle(.secondary) }
                if selected {
                    Text("In Use").foregroundStyle(Color.accentColor)
                } else if installed {
                    Text("Installed").foregroundStyle(.secondary)
                }
            }
            Text(Self.licence(m)).font(.footnote).foregroundStyle(m["licence"]["commercial"] == "yes" ? Color.secondary : .orange)
            if let speed = Self.speed(m) { Text(speed.0).font(.footnote).foregroundStyle(speed.1 ? Color.secondary : .orange) }
            if state == "running", let d = dl {
                ProgressView(value: d["bytes"].double ?? 0, total: max(d["total"].double ?? 1, 1)) {
                    Text("From \(m["downloadHost"].string ?? "")").font(.caption)
                }
            } else if state == "done" {
                Text("Downloaded and checked. Installing…").font(.footnote).foregroundStyle(.secondary)
            } else if state == "failed" {
                Text(dl?["error"].string ?? String(localized: "The download failed")).font(.footnote).foregroundStyle(.orange)
            }
            HStack(spacing: 12) {
                if installed {
                    if !selected && m["role"] != "detector" {
                        Button("Use") { act("faces.models.select", id) }
                    }
                    Button("Remove", role: .destructive) { act("faces.models.remove", id) }
                } else if state == "running" {
                    Button("Cancel") { act("faces.models.downloadCancel", id) }
                } else if state != "done" {
                    if runtime, m["downloadHost"].string?.isEmpty == false {
                        Button(state == "failed" ? "Try Again" : "Download") { offer = FaceModelOffer(download: m) }
                    }
                    if let page = m["source"].string.flatMap(URL.init(string:)) {
                        Link("Open Page", destination: page)
                    }
                }
            }
            .buttonStyle(.bordered)
            .font(.subheadline)
        }
        .padding(.vertical, 2)
    }

    private func act(_ command: String, _ id: String) {
        Task {
            await model.run(command, ["id": .string(id)])
            reload()
        }
    }

    /// A model file picked in Files: copied in (it may be in another app's place), then shown
    /// with its terms (`faces.models.inspect`); nothing is installed before they are accepted.
    private func inspect(_ url: URL) async {
        guard let engine = model.engine else { return }
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("FaceModels", isDirectory: true)
        let copy = dir.appendingPathComponent(url.lastPathComponent)
        do {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            try? FileManager.default.removeItem(at: copy)
            try FileManager.default.copyItem(at: url, to: copy)
            let info = try await engine.execute("faces.models.inspect", ["path": .string(copy.path)])
            offer = FaceModelOffer(info: info, path: copy.path)
        } catch {
            model.show(error.localizedDescription, error: true)
        }
    }

    static func mb(_ bytes: Double) -> String {
        bytes >= 1e6 ? String(format: "%.0f MB", bytes / 1e6) : String(format: "%.1f MB", bytes / 1e6)
    }

    /// "<licence> · commercial use allowed / non-commercial use only / terms unclear".
    static func licence(_ m: JSONValue) -> String {
        let name = m["licence"]["name"].string.flatMap { $0.isEmpty ? nil : $0 } ?? String(localized: "Unknown licence")
        let terms = switch m["licence"]["commercial"].string {
        case "yes": String(localized: "commercial use allowed")
        case "no": String(localized: "non-commercial use only")
        default: String(localized: "terms unclear")
        }
        return "\(name) · \(terms)"
    }

    /// How fast it is and whether it passed its last test (`false`: it failed).
    static func speed(_ m: JSONValue) -> (String, Bool)? {
        let speed = m["speedText"].string.flatMap { $0.isEmpty ? nil : $0 }
        let test = m["accepted"]["selfTest"]
        if !test.isNull {
            if test["ok"].bool == true { return (speed.map { String(localized: "Works · \($0)") } ?? String(localized: "Works"), true) }
            return (String(localized: "Failed its last test"), false)
        }
        return speed.map { ($0, true) }
    }
}

/// A face model to download (its terms first) or a file to install, as `faces.models.inspect`
/// describes it.
struct FaceModelOffer: Identifiable {
    let id = UUID()
    let info: JSONValue
    /// The file to install; `nil`: a download.
    let path: String?

    init(info: JSONValue, path: String) {
        self.info = info
        self.path = path
    }

    /// Pressing Download: the model's terms first.
    init(download m: JSONValue) {
        info = ["kind": "known", "fileName": m["id"], "sizeBytes": m["sizeBytes"], "model": m, "alreadyInstalled": false,
                "download": m["id"], "host": m["downloadHost"], "assumptions": [], "reason": .null]
        path = nil
    }
}

/// Add Face Model: what the file (or download) is, its licence and notice, what was trained on,
/// what LightCraft assumed about an unknown one, and the box to accept its terms; then Install or
/// Download (nothing is fetched or installed before).
struct FaceModelSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let offer: FaceModelOffer
    @State private var accepted = false

    var body: some View {
        let info = offer.info, m = info["model"]
        let kind = info["kind"].string ?? "unsupported"
        let download = info["download"].string
        NavigationStack {
            List {
                if kind == "unsupported" {
                    Section {
                        Text("This file cannot be used yet").font(.headline).foregroundStyle(.orange)
                        Text(info["reason"].string ?? String(localized: "It is not a face recognition model LightCraft understands"))
                        Text(verbatim: "\(info["fileName"].string ?? "") · \(FacesSettings.mb(info["sizeBytes"].double ?? 0))").foregroundStyle(.secondary)
                    }
                } else {
                    Section {
                        LabeledContent(m["name"].string ?? info["fileName"].string ?? "", value: FacesSettings.mb(info["sizeBytes"].double ?? 0))
                        if info["alreadyInstalled"].bool == true {
                            Text("Already installed. Installing again is harmless.").foregroundStyle(.secondary)
                        }
                        if download != nil {
                            Text(m["role"] == "detector"
                                ? "Downloads from \(info["host"].string ?? "its own repository"). Once it has arrived and checked out, LightCraft installs it."
                                : "Downloads from \(info["host"].string ?? "its own repository"). Once it has arrived and checked out, LightCraft installs it, starts using it and turns face recognition on.")
                        }
                    }
                    Section("Terms") {
                        Text(FacesSettings.licence(m)).fontWeight(.semibold).foregroundStyle(m["licence"]["commercial"] == "yes" ? Color.primary : .orange)
                        if let notice = m["licence"]["notice"].string, !notice.isEmpty { Text(notice) }
                        if let p = m["provenance"].string, !p.isEmpty { Text("Trained on: \(p)").foregroundStyle(.secondary) }
                        if kind == "draft" {
                            Text("LightCraft does not know this model, so it assumed:")
                            ForEach((info["assumptions"].array ?? []).compactMap(\.string), id: \.self) { Text(verbatim: "•  \($0)").font(.footnote) }
                        }
                        if let url = (m["source"].string ?? m["licence"]["url"].string).flatMap(URL.init(string:)) {
                            Link("Open the Model's Page", destination: url)
                        }
                    }
                    Section {
                        Toggle("I have read these terms and accept them for my own use", isOn: $accepted)
                    } footer: {
                        Text(download != nil
                            ? "Nothing is fetched until you accept. The model is kept on this device only; LightCraft never uploads or shares it."
                            : "The model is kept on this device only. LightCraft never uploads or shares it.")
                    }
                }
            }
            .navigationTitle("Add Face Model")
            .inlineTitle()
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel", systemImage: "xmark") { dismiss() } }
                if kind != "unsupported" {
                    ToolbarItem(placement: .confirmationAction) {
                        Button(download != nil ? "Download" : "Install") { confirm(download) }
                            .disabled(!accepted)
                    }
                }
            }
        }
    }

    private func confirm(_ download: String?) {
        Task {
            let done: JSONValue?
            if let download {
                done = await model.run("faces.models.download", ["id": .string(download), "acknowledged": true])
            } else if let path = offer.path {
                done = await model.run("faces.models.install", ["path": .string(path), "acknowledged": true])
            } else {
                done = nil
            }
            if done != nil { dismiss() }
        }
    }
}
