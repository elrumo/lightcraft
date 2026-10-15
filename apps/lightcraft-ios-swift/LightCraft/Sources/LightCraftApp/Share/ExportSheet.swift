import LightCraftCore
import Photos
import SwiftUI

#if os(iOS)
import UIKit
#else
import AppKit
#endif

/// Share: the photos chosen, then Share (the system's share sheet), Export As… (the options) and
/// Save to Photos. Files are written into the app's staging folder (`tmp/Exports`, emptied at every
/// launch) by the engine's exporter (`app.export`), the same as the desktop app's.
struct ExportSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let ids: [UInt64]
    @State private var options = ExportOptions()
    @State private var showOptions = false
    @State private var working = false
    @State private var sharing: [URL]?
    @State private var presets: [JSONValue] = []
    @State private var savingPreset = false
    @State private var presetName = ""

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    ScrollView(.horizontal) {
                        HStack(spacing: 6) {
                            ForEach(ids.prefix(30), id: \.self) { id in
                                ExportThumb(photo: id)
                            }
                        }
                    }
                    .scrollIndicators(.hidden)
                    .listRowInsets(EdgeInsets(top: 8, leading: 12, bottom: 8, trailing: 12))
                } footer: {
                    Text(ids.count == 1 ? String(localized: "1 photo") : String(localized: "\(ids.count) photos"))
                }
                Section {
                    Button {
                        Task { await run(.share) }
                    } label: {
                        Label("Share", systemImage: "square.and.arrow.up")
                    }
                    Button {
                        Task { await run(.photos) }
                    } label: {
                        Label("Save to Photos", systemImage: "square.and.arrow.down")
                    }
                    .disabled(!options.photosCanTake)
                }
                .disabled(ids.isEmpty || working)
                Section("Options") {
                    // a preset fills the options in; what it holds beyond them (colour space,
                    // sharpening…) goes with the export
                    Menu {
                        ForEach(presets, id: \.self) { p in
                            Button(p["name"].string ?? "") { options.load(p) }
                        }
                        Divider()
                        Button("Save as Preset…", systemImage: "plus") { savingPreset = true }
                    } label: {
                        LabeledContent("Preset", value: options.preset ?? String(localized: "Choose…"))
                    }
                    Picker("File Type", selection: $options.format) {
                        ForEach(ExportOptions.formats, id: \.0) { Text($0.1).tag($0.0) }
                    }
                    Picker("Size", selection: $options.longEdge) {
                        Text("Small (1080 px)").tag(1080)
                        Text("Medium (2048 px)").tag(2048)
                        Text("Large (4096 px)").tag(4096)
                        Text("Full Size").tag(0)
                    }
                    .disabled(options.format == "original" || options.format == "dng")
                    if options.format == "jpeg" || options.format == "avif" || options.format == "webp" {
                        Picker("Quality", selection: $options.quality) {
                            ForEach([100, 95, 90, 85, 80, 70, 60, 50], id: \.self) { Text("\($0) %").tag($0) }
                        }
                    }
                    Toggle("Include Watermark", isOn: $options.watermark)
                    if options.watermark {
                        TextField("© Your Name", text: $options.watermarkText)
                    }
                    Picker("Metadata", selection: $options.metadata) {
                        Text("All").tag("all")
                        Text("All Except Camera").tag("allExceptCamera")
                        Text("Copyright Only").tag("copyright")
                        Text("None").tag("none")
                    }
                    Toggle("Remove Location", isOn: $options.removeLocation)
                    NavigationLink("More Options") { ExportMoreOptions(options: $options) }
                }
                if working {
                    Section {
                        ExportProgress()
                    }
                }
            }
            .navigationTitle("Share")
            .inlineTitle()
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close", systemImage: "xmark") { dismiss() }
                }
            }
            .alert("Save Export Preset", isPresented: $savingPreset) {
                TextField("Preset Name", text: $presetName)
                Button("Save") { savePreset() }
                Button("Cancel", role: .cancel) {}
            }
            .task { await loadPresets() }
            .sheet(item: Binding(get: { sharing.map(ShareItems.init) }, set: { if $0 == nil { sharing = nil } })) { items in
                ActivityView(urls: items.urls) { dismiss() }
                    .ignoresSafeArea()
            }
        }
        .presentationDetents([.medium, .large])
    }

    private enum Destination { case share, photos }

    private func loadPresets() async {
        guard let engine = model.engine else { return }
        presets = (try? await engine.execute("export.presets"))?.array ?? []
    }

    private func savePreset() {
        let name = presetName.trimmingCharacters(in: .whitespaces)
        guard !name.isEmpty else { return }
        Task {
            if await model.run("export.savePreset", ["name": .string(name), "params": .object(options.params)]) != nil {
                options.preset = name
                presetName = ""
                model.show(String(localized: "Saved export preset “\(name)”"))
                await loadPresets()
            }
        }
    }

    private func run(_ to: Destination) async {
        working = true
        defer { working = false }
        let dir = URL(fileURLWithPath: NSTemporaryDirectory()).appending(path: "Exports").appending(path: "x-\(UUID().uuidString.prefix(8))")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        var params = options.params
        params["dir"] = .string(dir.path)
        params["ids"] = AppModel.ids(ids)
        params["conflict"] = "unique"
        guard let r = await model.run("app.export", .object(params)) else { return }
        let urls = (r["files"].array ?? []).compactMap { $0["path"].string }.map { URL(fileURLWithPath: $0) }
        let failed = (r["files"].array ?? []).compactMap { $0["error"].string }
        if r["cancelled"].bool == true {
            model.show(String(localized: "Export cancelled"))
            if urls.isEmpty { return }
        } else if let first = failed.first {
            model.show(String(localized: "\(failed.count) failed: \(first)"), error: true)
        }
        guard !urls.isEmpty else { return }
        switch to {
        case .share:
            sharing = urls
        case .photos:
            do {
                try await Self.saveToPhotos(urls)
                model.show(urls.count == 1 ? String(localized: "Saved to Photos") : String(localized: "Saved \(urls.count) photos to Photos"))
                dismiss()
            } catch {
                model.show(error.localizedDescription, error: true)
            }
        }
    }

    /// Add the files to the photo library (add-only access: iOS asks once).
    static func saveToPhotos(_ urls: [URL]) async throws {
        let status = await PHPhotoLibrary.requestAuthorization(for: .addOnly)
        guard status == .authorized || status == .limited else {
            throw ErrorBody(code: "denied", message: String(localized: "LightCraft may not add to your photo library: allow it in Settings ▸ Privacy ▸ Photos."))
        }
        try await PHPhotoLibrary.shared().performChanges {
            for url in urls {
                PHAssetCreationRequest.forAsset().addResource(with: .photo, fileURL: url, options: nil)
            }
        }
    }
}

/// The export running: how far it is, the file in hand, and Cancel (its activity row).
struct ExportProgress: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let row = model.activity.first { $0.kind == "export" }
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text(row.map { $0.total > 0 ? String(localized: "Exporting \($0.done) of \($0.total)") : String(localized: "Exporting…") } ?? String(localized: "Exporting…"))
                    .font(.headline)
                Spacer()
                if let row, row.cancellable {
                    Button(row.cancelling ? "Stopping…" : "Cancel", role: .destructive) {
                        model.perform("activity.cancel", ["id": .int(Int64(clamping: row.id))])
                    }
                    .disabled(row.cancelling)
                }
            }
            if let row, row.total > 0 {
                ProgressView(value: Double(row.done), total: Double(row.total))
            } else {
                ProgressView()
            }
            if let detail = row?.detail, !detail.isEmpty {
                Text(detail).font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
        }
    }
}

/// The options the Share page offers, as `app.export` takes them.
struct ExportOptions {
    var format = "jpeg"
    var longEdge = 2048
    var quality = 90
    var watermark = false
    var watermarkText = ""
    var metadata = "all"
    var removeLocation = false
    /// The export preset the options came from (`app.export {preset}`: what it holds beyond them goes too).
    var preset: String?
    // More Options (app.export's own names and values)
    var colorSpace = "srgb"
    var bitDepth = 0
    var hdr = false
    var sharpen = "none"
    var sharpenAmount = "standard"
    var tiffCompression = "Deflate"
    var dngCompression = "lossless"
    var jxlLossless = false
    var jxlEffort = "normal"
    var jxlProgressive = false
    var naming = ""

    /// Fill the options in from an export preset (`export.presets`): what this form shows of it.
    mutating func load(_ p: JSONValue) {
        let params = p["params"]
        preset = p["name"].string
        if let f = params["format"].string { format = f }
        if let q = params["quality"].int { quality = Int(q) }
        if let e = params["longEdge"].int { longEdge = Int(e) } else if !params["resize"].isNull { longEdge = Int(params["resize"]["value"].int ?? Int64(longEdge)) }
        if let m = params["metadata"].string { metadata = m }
        if let r = params["removeLocation"].bool { removeLocation = r }
        let wm = params["watermark"].string ?? params["watermark"]["text"].string
        watermark = wm?.isEmpty == false
        watermarkText = wm ?? ""
        colorSpace = params["colorSpace"].string ?? "srgb"
        bitDepth = Int(params["bitDepth"].int ?? 0)
        hdr = params["hdr"].bool ?? false
        sharpen = params["sharpen"].string ?? "none"
        sharpenAmount = params["sharpenAmount"].string ?? "standard"
        tiffCompression = params["tiffCompression"].string ?? "Deflate"
        dngCompression = params["dngCompression"].string ?? "lossless"
        jxlLossless = params["jxlLossless"].bool ?? false
        jxlEffort = params["jxlEffort"].string ?? "normal"
        jxlProgressive = params["jxlProgressive"].bool ?? false
        naming = params["naming"].string ?? ""
    }

    static let formats: [(String, String)] = [
        ("jpeg", "JPEG"), ("png", "PNG"), ("tiff", "TIFF"), ("psd", "PSD"), ("webp", "WebP"), ("avif", "AVIF"), ("jxl", "JPEG XL"), ("dng", "DNG"),
        ("original", "Original"),
    ]

    /// Photos takes JPEG, PNG, TIFF and DNG.
    var photosCanTake: Bool { ["jpeg", "png", "tiff", "dng"].contains(format) }

    var params: [String: JSONValue] {
        var p: [String: JSONValue] = ["format": .string(format), "metadata": .string(metadata), "removeLocation": .bool(removeLocation)]
        if let preset { p["preset"] = .string(preset) }
        if format != "original" && format != "dng" { p["longEdge"] = .int(Int64(longEdge)) }
        if ["jpeg", "avif", "webp"].contains(format) { p["quality"] = .int(Int64(quality)) }
        if watermark && !watermarkText.isEmpty { p["watermark"] = .string(watermarkText) }
        // More Options, where they apply
        if format != "original" && format != "dng" {
            p["colorSpace"] = .string(colorSpace)
            p["sharpen"] = .string(sharpen)
            p["sharpenAmount"] = .string(sharpenAmount)
        }
        if bitDepth > 0 && ["png", "tiff", "psd"].contains(format) { p["bitDepth"] = .int(Int64(bitDepth)) }
        if hdr && ["png", "tiff", "avif", "jxl"].contains(format) { p["hdr"] = true }
        if format == "tiff" { p["tiffCompression"] = .string(tiffCompression) }
        if format == "dng" { p["dngCompression"] = .string(dngCompression) }
        if format == "jxl" {
            p["jxlLossless"] = .bool(jxlLossless)
            p["jxlEffort"] = .string(jxlEffort)
            p["jxlProgressive"] = .bool(jxlProgressive)
        }
        if !naming.trimmingCharacters(in: .whitespaces).isEmpty { p["naming"] = .string(naming) }
        return p
    }
}

/// More Options: the colour space and bit depth, HDR, JPEG XL's options, output sharpening,
/// compression and the files' names.
private struct ExportMoreOptions: View {
    @Binding var options: ExportOptions

    var body: some View {
        let f = options.format
        Form {
            if f != "original" && f != "dng" {
                Section("Color") {
                    Picker("Color Space", selection: $options.colorSpace) {
                        Text("sRGB").tag("srgb")
                        Text("Display P3").tag("displayP3")
                        Text("Adobe RGB").tag("adobeRgb")
                        Text("ProPhoto RGB").tag("proPhoto")
                        Text("Rec. 2020").tag("rec2020")
                    }
                    if ["png", "tiff", "psd"].contains(f) {
                        Picker("Bit Depth", selection: $options.bitDepth) {
                            Text("Automatic").tag(0)
                            Text("8 bits").tag(8)
                            Text("16 bits").tag(16)
                            if f == "tiff" { Text("32 bits (float)").tag(32) }
                        }
                    }
                    if ["png", "tiff", "avif", "jxl"].contains(f) {
                        Toggle("HDR Output", isOn: $options.hdr)
                    }
                }
                Section("Output Sharpening") {
                    Picker("Sharpen For", selection: $options.sharpen) {
                        Text("None").tag("none")
                        Text("Screen").tag("screen")
                        Text("Matte Paper").tag("matte")
                        Text("Glossy Paper").tag("glossy")
                    }
                    if options.sharpen != "none" {
                        Picker("Amount", selection: $options.sharpenAmount) {
                            Text("Low").tag("low")
                            Text("Standard").tag("standard")
                            Text("High").tag("high")
                        }
                    }
                }
            }
            if f == "jxl" {
                Section("JPEG XL") {
                    Toggle("Lossless", isOn: $options.jxlLossless)
                    Picker("Effort", selection: $options.jxlEffort) {
                        Text("Fast").tag("fast")
                        Text("Normal").tag("normal")
                    }
                    Toggle("Progressive", isOn: $options.jxlProgressive)
                }
            }
            if f == "tiff" {
                Section("Compression") {
                    Picker("Compression", selection: $options.tiffCompression) {
                        Text("None").tag("None")
                        Text("LZW").tag("Lzw")
                        Text("ZIP").tag("Deflate")
                    }
                }
            }
            if f == "dng" {
                Section("Compression") {
                    Picker("Compression", selection: $options.dngCompression) {
                        Text("Lossless").tag("lossless")
                        Text("ZIP").tag("deflate")
                        Text("None").tag("uncompressed")
                    }
                }
            }
            Section {
                TextField("{name}", text: $options.naming)
                    .noAutocap()
                    .autocorrectionDisabled()
            } header: {
                Text("File Name")
            } footer: {
                Text("Empty keeps the photo's name. {name}, {seq}, {date}, {camera}, {lens}, {iso}, {rating}, {title} are filled in.")
            }
        }
        .navigationTitle("More Options")
        .inlineTitle()
    }
}

private struct ShareItems: Identifiable {
    let urls: [URL]
    var id: String { urls.map(\.path).joined() }
}

#if os(iOS)
/// The system's share sheet for files.
struct ActivityView: UIViewControllerRepresentable {
    let urls: [URL]
    let done: () -> Void

    func makeUIViewController(context: Context) -> UIActivityViewController {
        let vc = UIActivityViewController(activityItems: urls, applicationActivities: nil)
        vc.completionWithItemsHandler = { _, completed, _, _ in
            if completed { done() }
        }
        return vc
    }

    func updateUIViewController(_ vc: UIActivityViewController, context: Context) {}
}
#else
/// A Mac's sheet for files that were written for sharing: the system's share menu (ShareLink),
/// Show in Finder, or done. (The Mac export flow proper, with its own destination, is SM3.)
struct ActivityView: View {
    let urls: [URL]
    let done: () -> Void

    var body: some View {
        VStack(spacing: 18) {
            Image(systemName: "square.and.arrow.up").font(.largeTitle).foregroundStyle(.secondary)
            Text(urls.count == 1 ? (urls.first?.lastPathComponent ?? "") : "\(urls.count) files")
                .font(.headline)
                .lineLimit(2)
                .multilineTextAlignment(.center)
            HStack {
                ShareLink(items: urls) { Label("Share", systemImage: "square.and.arrow.up") }
                Button("Show in Finder") { NSWorkspace.shared.activateFileViewerSelecting(urls) }
                Button("Done", action: done).keyboardShortcut(.defaultAction)
            }
        }
        .padding(28)
        .frame(minWidth: 380)
    }
}
#endif

private struct ExportThumb: View {
    @Environment(AppModel.self) private var model
    let photo: UInt64

    var body: some View {
        Color.lcTertiaryFill
            .frame(width: 72, height: 72)
            .overlay {
                if let p = model.images.thumb(photo) {
                    Image(decorative: p.image, scale: 1).resizable().scaledToFill()
                }
            }
            .clipShape(.rect(cornerRadius: 8))
            .task { model.wantThumb(photo, edge: 256) }
            .onDisappear { model.thumbDisappeared(photo) }
    }
}
