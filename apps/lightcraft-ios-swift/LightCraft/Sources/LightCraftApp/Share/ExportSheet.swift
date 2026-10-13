#if os(iOS)
import LightCraftCore
import Photos
import SwiftUI
import UIKit

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
                }
                if working {
                    Section {
                        ExportProgress()
                    }
                }
            }
            .navigationTitle("Share")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close", systemImage: "xmark") { dismiss() }
                }
            }
            .sheet(item: Binding(get: { sharing.map(ShareItems.init) }, set: { if $0 == nil { sharing = nil } })) { items in
                ActivityView(urls: items.urls) { dismiss() }
                    .ignoresSafeArea()
            }
        }
        .presentationDetents([.medium, .large])
    }

    private enum Destination { case share, photos }

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

    static let formats: [(String, String)] = [
        ("jpeg", "JPEG"), ("png", "PNG"), ("tiff", "TIFF"), ("webp", "WebP"), ("avif", "AVIF"), ("jxl", "JPEG XL"), ("dng", "DNG"), ("original", "Original"),
    ]

    /// Photos takes JPEG, PNG, TIFF and DNG.
    var photosCanTake: Bool { ["jpeg", "png", "tiff", "dng"].contains(format) }

    var params: [String: JSONValue] {
        var p: [String: JSONValue] = ["format": .string(format), "metadata": .string(metadata), "removeLocation": .bool(removeLocation)]
        if format != "original" && format != "dng" { p["longEdge"] = .int(Int64(longEdge)) }
        if ["jpeg", "avif", "webp"].contains(format) { p["quality"] = .int(Int64(quality)) }
        if watermark && !watermarkText.isEmpty { p["watermark"] = .string(watermarkText) }
        return p
    }
}

private struct ShareItems: Identifiable {
    let urls: [URL]
    var id: String { urls.map(\.path).joined() }
}

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

private struct ExportThumb: View {
    @Environment(AppModel.self) private var model
    let photo: UInt64

    var body: some View {
        Color(uiColor: .tertiarySystemFill)
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
#endif
