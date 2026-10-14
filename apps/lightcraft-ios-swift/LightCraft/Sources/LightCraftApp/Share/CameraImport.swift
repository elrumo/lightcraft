#if os(iOS)
import CoreGraphics
import ImageIO
import LightCraftCore
import SwiftUI
@preconcurrency import ImageCaptureCore

/// The cameras, phones and card readers plugged in (ImageCaptureCore, as egui's `crates/capture`
/// reads them), and the one whose photos are being reviewed: its files (`CameraFile`s for the
/// engine's `library.cameraFiles`), their thumbnails, and copying the chosen ones off it. Everything
/// here runs on the main thread, where ImageCaptureCore calls back.
@Observable
final class Cameras: NSObject {
    /// What is plugged in.
    private(set) var devices: [ICCameraDevice] = []
    /// The one being reviewed.
    private(set) var open: ICCameraDevice?
    /// Its files, in its order, by the id given each.
    private(set) var order: [String] = []
    /// It has said it has listed everything.
    private(set) var complete = false
    private(set) var error: String?
    private(set) var thumbnails: [String: CGImage] = [:]
    /// A device was plugged in (the app offers its import).
    @ObservationIgnored var connected: ((ICCameraDevice) -> Void)?
    @ObservationIgnored private let browser = ICDeviceBrowser()
    @ObservationIgnored private var files: [String: ICCameraFile] = [:]
    @ObservationIgnored private var ids: [ObjectIdentifier: String] = [:]
    @ObservationIgnored private var asked: Set<String> = []
    @ObservationIgnored private var started = false

    /// Thumbnails kept at once (a card of two thousand photos mustn't hold them all in memory).
    private static let maxThumbnails = 240

    /// Watch for devices (from launch: the import can open when one is plugged in).
    func start() {
        guard !started else { return }
        started = true
        browser.delegate = self
        if let mask = ICDeviceTypeMask(rawValue: ICDeviceTypeMask.camera.rawValue | ICDeviceLocationTypeMask.local.rawValue) {
            browser.browsedDeviceTypeMask = mask
        }
        browser.start()
    }

    /// Read `device`'s photos (asking first, once, for permission to).
    func read(_ device: ICCameraDevice) {
        guard device !== open else { return }
        close()
        open = device
        device.delegate = self
        if browser.contentsAuthorizationStatus == .authorized {
            device.requestOpenSession()
        } else {
            Self.authorize(browser) { [weak self] granted in
                Task { @MainActor in
                    guard let self else { return }
                    if granted {
                        self.open?.requestOpenSession()
                    } else {
                        self.error = String(localized: "LightCraft isn't allowed to read cameras and cards: allow it in Settings ▸ Privacy & Security")
                    }
                }
            }
        }
    }

    /// Done with the device being reviewed.
    func close() {
        open?.requestCloseSession()
        open = nil
        order = []
        files = [:]
        ids = [:]
        asked = []
        thumbnails = [:]
        complete = false
        error = nil
    }

    /// The files, as the engine's review takes them (`CameraFile`).
    var cameraFiles: [JSONValue] {
        order.compactMap { id in files[id].map { Self.describe($0, id: id) } }
    }

    /// Ask for a file's thumbnail (once; it comes in `thumbnails`).
    func wantThumbnail(_ id: String) {
        guard thumbnails[id] == nil, !asked.contains(id), let file = files[id] else { return }
        asked.insert(id)
        Self.thumbnail(of: file) { [weak self] image in
            Task { @MainActor in
                guard let self, let image else { return }
                if self.thumbnails.count >= Self.maxThumbnails, let gone = self.thumbnails.keys.first {
                    self.thumbnails[gone] = nil
                    self.asked.remove(gone)
                }
                self.thumbnails[id] = image
            }
        }
    }

    /// Copy a file off the device into `dir`: the path of its copy.
    func download(_ id: String, to dir: URL) async throws -> String {
        guard let file = files[id], let name = file.name else { throw CameraError(text: String(localized: "No photos found")) }
        let dest = ImportModel.unique(name, in: dir)
        return try await withCheckedThrowingContinuation { done in
            Self.download(file, to: dir, as: dest.lastPathComponent) { saved, error in
                if let error {
                    done.resume(throwing: error)
                } else {
                    done.resume(returning: dir.appending(path: saved ?? dest.lastPathComponent).path)
                }
            }
        }
    }

    struct CameraError: LocalizedError {
        let text: String
        var errorDescription: String? { text }
    }

    // MARK: the device's own calls (any thread: they hop back here)

    nonisolated private static func authorize(_ browser: ICDeviceBrowser, _ done: @escaping @Sendable (Bool) -> Void) {
        browser.requestContentsAuthorization { status in done(status == .authorized) }
    }

    nonisolated private static func thumbnail(of file: ICCameraFile, _ done: @escaping @Sendable (CGImage?) -> Void) {
        file.requestThumbnailData(options: [.imageSourceThumbnailMaxPixelSize: 256]) { data, _ in
            guard let data, let source = CGImageSourceCreateWithData(data as CFData, nil) else { return done(nil) }
            done(CGImageSourceCreateImageAtIndex(source, 0, nil))
        }
    }

    nonisolated private static func download(_ file: ICCameraFile, to dir: URL, as name: String, _ done: @escaping @Sendable (String?, Error?) -> Void) {
        _ = file.requestDownload(options: [.downloadsDirectoryURL: dir, .saveAsFilename: name, .overwrite: true]) { saved, error in
            done(saved, error)
        }
    }

    /// A file as the engine's `CameraFile`: when it was taken in local time, as the library keeps
    /// capture times (from its EXIF date when the device knows it).
    private static func describe(_ f: ICCameraFile, id: String) -> JSONValue {
        let date = f.exifCreationDate ?? f.creationDate ?? f.fileCreationDate
        var v: [String: JSONValue] = [
            "id": .string(id), "name": .string(f.name ?? id), "size": .int(Int64(f.fileSize)),
            "exifDated": .bool(f.exifCreationDate != nil), "raw": .bool(f.isRaw),
            "width": .int(Int64(max(f.width, 0))), "height": .int(Int64(max(f.height, 0))),
        ]
        if let date { v["captured"] = .string(captureFormat.string(from: date)) }
        return .object(v)
    }

    private static let captureFormat: DateFormatter = {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "yyyy-MM-dd'T'HH:mm:ss"
        return f
    }()

    /// The device's files, again (it lists them as it reads its card).
    private func catalogue(_ camera: ICCameraDevice) {
        guard camera === open else { return }
        var next: [String] = []
        for case let file as ICCameraFile in camera.mediaFiles ?? [] {
            let key = ObjectIdentifier(file)
            let id = ids[key] ?? "f\(ids.count)"
            ids[key] = id
            files[id] = file
            next.append(id)
        }
        order = next
    }
}

extension Cameras: @preconcurrency ICDeviceBrowserDelegate {
    func deviceBrowser(_ browser: ICDeviceBrowser, didAdd device: ICDevice, moreComing: Bool) {
        guard let camera = device as? ICCameraDevice, !devices.contains(where: { $0 === camera }) else { return }
        devices.append(camera)
        connected?(camera)
    }

    func deviceBrowser(_ browser: ICDeviceBrowser, didRemove device: ICDevice, moreGoing: Bool) {
        devices.removeAll { $0 === device }
        if device === open { close() }
    }
}

extension Cameras: @preconcurrency ICCameraDeviceDelegate {
    func device(_ device: ICDevice, didOpenSessionWithError error: (any Error)?) {
        if let error, device === open { self.error = error.localizedDescription }
    }

    func device(_ device: ICDevice, didCloseSessionWithError error: (any Error)?) {}

    func didRemove(_ device: ICDevice) {
        devices.removeAll { $0 === device }
        if device === open { close() }
    }

    func cameraDevice(_ camera: ICCameraDevice, didAdd items: [ICCameraItem]) { catalogue(camera) }

    func cameraDevice(_ camera: ICCameraDevice, didRemove items: [ICCameraItem]) { catalogue(camera) }

    func cameraDevice(_ camera: ICCameraDevice, didReceiveThumbnail thumbnail: CGImage?, for item: ICCameraItem, error: (any Error)?) {}

    func cameraDevice(_ camera: ICCameraDevice, didReceiveMetadata metadata: [AnyHashable: Any]?, for item: ICCameraItem, error: (any Error)?) {}

    func cameraDevice(_ camera: ICCameraDevice, didRenameItems items: [ICCameraItem]) { catalogue(camera) }

    func cameraDeviceDidChangeCapability(_ camera: ICCameraDevice) {}

    func cameraDevice(_ camera: ICCameraDevice, didReceivePTPEvent eventData: Data) {}

    func deviceDidBecomeReady(withCompleteContentCatalog device: ICCameraDevice) {
        catalogue(device)
        if device === open { complete = true }
    }

    func cameraDeviceDidRemoveAccessRestriction(_ device: ICDevice) {}

    func cameraDeviceDidEnableAccessRestriction(_ device: ICDevice) {
        if device === open { error = String(localized: "The camera is locked: unlock it to read its photos") }
    }
}

/// Add Photos ▸ Connected Camera/Card: the device's photos by day, newest first, the ones the
/// library already has greyed out; the chosen ones are copied off it and moved into the library.
struct CameraImportSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    /// The engine's review of the device's files (`library.cameraFiles`).
    @State private var review: [JSONValue] = []
    @State private var chosen: Set<String> = []
    /// Files already offered: new ones arrive chosen, these keep what was chosen for them.
    @State private var seen: Set<String> = []
    @State private var copying: (done: Int, total: Int)?
    @State private var stop = false

    var body: some View {
        let cameras = model.cameras
        NavigationStack {
            content(cameras)
                .navigationTitle(cameras.open?.name ?? String(localized: "Connected Camera/Card"))
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button(role: .close) {
                            if copying != nil { stop = true } else { dismiss() }
                        }
                    }
                    if cameras.devices.count > 1 {
                        ToolbarItem(placement: .topBarTrailing) {
                            Menu("Device", systemImage: "sdcard") {
                                ForEach(cameras.devices, id: \.self) { d in
                                    Button(d.name ?? "") { cameras.read(d) }
                                }
                            }
                        }
                    }
                    if !review.isEmpty {
                        ToolbarItem(placement: .confirmationAction) {
                            Button("Import \(chosen.count) Photos") { Task { await start() } }
                                .disabled(chosen.isEmpty || copying != nil)
                        }
                    }
                }
                .safeAreaBar(edge: .bottom) { footer }
        }
        .task {
            if cameras.open == nil, let first = cameras.devices.first { cameras.read(first) }
        }
        .task(id: Key(count: cameras.order.count, complete: cameras.complete)) {
            guard let engine = model.engine, !cameras.order.isEmpty else { return review = [] }
            let r = (try? await engine.execute("library.cameraFiles", ["files": .array(cameras.cameraFiles)]))?["files"].array ?? []
            review = r
            // what's new arrives chosen, when the library hasn't got it
            for f in r {
                guard let id = f["id"].string, !seen.contains(id) else { continue }
                seen.insert(id)
                if f["existing"].isNull { chosen.insert(id) }
            }
        }
        .onDisappear { cameras.close() }
        .interactiveDismissDisabled(copying != nil)
    }

    private nonisolated struct Key: Hashable {
        let count: Int
        let complete: Bool
    }

    @ViewBuilder private func content(_ cameras: Cameras) -> some View {
        if cameras.devices.isEmpty {
            ContentUnavailableView("No Camera or Card Found", systemImage: "sdcard", description: Text("Plug in a camera or a card reader (USB-C or Lightning), then try again."))
        } else if let error = cameras.error {
            ContentUnavailableView("Can't Read It", systemImage: "exclamationmark.triangle", description: Text(error))
        } else if let copying {
            VStack(spacing: 12) {
                ProgressView(value: Double(copying.done), total: Double(max(copying.total, 1)))
                HStack(spacing: 4) {
                    Text("Copying from camera")
                    Text(verbatim: "\(copying.done)/\(copying.total)").monospacedDigit()
                }
            }
            .padding(32)
        } else if review.isEmpty {
            if cameras.complete {
                ContentUnavailableView("No photos on it", systemImage: "photo.on.rectangle")
            } else {
                ProgressView(String(localized: "Reading \(cameras.open?.name ?? "")… \(cameras.order.count) photos so far"))
            }
        } else {
            ScrollView {
                LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 2), count: 3), spacing: 2, pinnedViews: [.sectionHeaders]) {
                    ForEach(days, id: \.day) { group in
                        Section {
                            ForEach(group.files, id: \.self) { f in cell(f, cameras) }
                        } header: {
                            dayHeader(group)
                        }
                    }
                }
            }
        }
    }

    /// The files by day (the engine gave them newest day first).
    private var days: [(day: String, files: [JSONValue])] {
        var out: [(day: String, files: [JSONValue])] = []
        for f in review {
            let day = f["day"].string ?? ""
            if out.last?.day == day {
                out[out.count - 1].files.append(f)
            } else {
                out.append((day, [f]))
            }
        }
        return out
    }

    private func dayHeader(_ group: (day: String, files: [JSONValue])) -> some View {
        let importable = group.files.filter { $0["existing"].isNull }.compactMap { $0["id"].string }
        let all = !importable.isEmpty && importable.allSatisfy(chosen.contains)
        return HStack {
            Text(group.day.isEmpty ? String(localized: "No Date") : InfoPanel.captureDate(group.day)?.formatted(date: .complete, time: .omitted) ?? group.day)
                .font(.subheadline.weight(.semibold))
            Text(verbatim: "· \(group.files.count)").font(.subheadline).foregroundStyle(.secondary)
            Spacer()
            if !importable.isEmpty {
                Button(all ? "Deselect All" : "Select All") {
                    if all { chosen.subtract(importable) } else { chosen.formUnion(importable) }
                }
                .font(.subheadline)
            }
        }
        .padding(.horizontal, 16)
        .frame(height: 40)
        .background(.bar)
    }

    private func cell(_ f: JSONValue, _ cameras: Cameras) -> some View {
        let id = f["id"].string ?? ""
        let existing = !f["existing"].isNull
        let on = chosen.contains(id)
        return Color(uiColor: .secondarySystemBackground)
            .aspectRatio(1, contentMode: .fit)
            .overlay {
                if let image = cameras.thumbnails[id] {
                    Image(decorative: image, scale: 1).resizable().scaledToFill()
                }
            }
            .clipped()
            .overlay { if existing { Color.black.opacity(0.55) } }
            .overlay(alignment: .bottomLeading) {
                HStack(spacing: 4) {
                    if f["raw"].bool == true { tag("Raw") }
                    if existing { tag("In library") }
                }
                .padding(4)
            }
            .overlay(alignment: .bottomTrailing) {
                if !existing { SelectionMark(chosen: on) }
            }
            .contentShape(Rectangle())
            .onTapGesture {
                guard !existing else { return }
                if on { chosen.remove(id) } else { chosen.insert(id) }
                model.buzz(.selection)
            }
            .onAppear { cameras.wantThumbnail(id) }
            .accessibilityElement()
            .accessibilityLabel(f["name"].string ?? "")
            .accessibilityValue(existing ? Text("In library") : Text(on ? "Selected" : ""))
            .accessibilityAddTraits(on ? [.isButton, .isSelected] : .isButton)
    }

    private func tag(_ text: LocalizedStringKey) -> some View {
        Text(text).font(.caption2.weight(.semibold)).padding(.horizontal, 5).padding(.vertical, 2).glassEffect(.regular, in: .capsule)
    }

    private var footer: some View {
        let dups = review.filter { !$0["existing"].isNull }.count
        return VStack(spacing: 4) {
            if !review.isEmpty {
                HStack(spacing: 4) {
                    Text("\(review.count) found · \(chosen.count) selected")
                    if dups > 0 { Text("· \(dups) already in the library") }
                }
            }
            Text("The photos are copied into the library; the originals on the camera or card are left as they are.")
                .foregroundStyle(.secondary)
        }
        .font(.footnote)
        .multilineTextAlignment(.center)
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
    }

    /// Copy the chosen files off the device (in its order), then move them into the library.
    private func start() async {
        let cameras = model.cameras
        let ids = review.compactMap { $0["id"].string }.filter(chosen.contains)
        guard !ids.isEmpty, let dir = try? ImportModel.batchDir() else { return }
        let name = cameras.open?.name ?? ""
        stop = false
        copying = (0, ids.count)
        var paths: [String] = []
        for id in ids {
            if stop { break }
            if let path = try? await cameras.download(id, to: dir) { paths.append(path) }
            copying = (paths.count, ids.count)
        }
        copying = nil
        dismiss()
        if stop, paths.isEmpty {
            model.show(String(localized: "Import cancelled"))
            return
        }
        if paths.isEmpty {
            model.show(String(localized: "No photos could be copied from \(name)"), error: true)
            return
        }
        if paths.count < ids.count, !stop {
            model.show(String(localized: "Photos not copied from \(name): \(ids.count - paths.count)"), error: true)
        }
        await model.importChosen(["paths": .array(paths.map { .string($0) }), "mode": "move"])
    }
}
#endif
