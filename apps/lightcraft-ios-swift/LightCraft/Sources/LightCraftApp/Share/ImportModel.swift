import Foundation
import ImageIO
import LightCraftCore
import Observation
import PhotosUI
import SwiftUI
import UniformTypeIdentifiers

/// Adding photos from the system's pickers: the files chosen are copied into a batch folder of the
/// app's staging area (`tmp/Import/<batch>`, emptied at every launch) and the engine moves them into
/// the library (`library.import {mode: move}`): originals in Photos and Files are never touched.
@Observable
final class ImportModel {
    var pickingPhotos = false
    var pickingFiles = false
    var pickingFolder = false
    var picked: [PhotosPickerItem] = []
    /// Copying from the picker in progress (`done` of `total`).
    private(set) var copying: (done: Int, total: Int)?

    /// What the Files picker offers: pictures, raws and DNGs.
    static let fileTypes: [UTType] = [.image, .rawImage, .tiff, .heic, .heif, .jpeg, .png]
    /// Most files one folder pick brings in, and how deep it looks.
    nonisolated static let maxFolderFiles = 5000
    nonisolated static let maxFolderDepth = 12

    /// A new batch folder in the staging area.
    nonisolated static func batchDir() throws -> URL {
        let dir = URL(fileURLWithPath: NSTemporaryDirectory()).appending(path: "Import").appending(path: "pick-\(UUID().uuidString.prefix(8))")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    /// `name` in `dir`, numbered when taken (`IMG_0001 2.jpg`).
    nonisolated static func unique(_ name: String, in dir: URL) -> URL {
        let base = (name as NSString).deletingPathExtension
        let ext = (name as NSString).pathExtension
        var url = dir.appending(path: name)
        var n = 2
        while FileManager.default.fileExists(atPath: url.path) {
            url = dir.appending(path: ext.isEmpty ? "\(base) \(n)" : "\(base) \(n).\(ext)")
            n += 1
        }
        return url
    }

    /// Photos chosen in the Photos picker: their originals (HEIC, ProRAW…) copied out, then imported.
    func photos(_ items: [PhotosPickerItem], _ model: AppModel) {
        guard !items.isEmpty else { return }
        picked = []
        Task {
            copying = (0, items.count)
            var paths: [String] = []
            var failed = 0
            for item in items {
                if let file = try? await item.loadTransferable(type: StagedFile.self) {
                    paths.append(file.url.path)
                } else {
                    failed += 1
                }
                copying = ((copying?.done ?? 0) + 1, items.count)
            }
            copying = nil
            await importStaged(paths, failed: failed, model)
        }
    }

    /// Files chosen in the Files picker.
    func files(_ result: Result<[URL], Error>, _ model: AppModel) {
        switch result {
        case .failure(let e): model.show(e.localizedDescription, error: true)
        case .success(let urls):
            Task {
                do {
                    let dir = try Self.batchDir()
                    var paths: [String] = []
                    for url in urls {
                        let scoped = url.startAccessingSecurityScopedResource()
                        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
                        let dest = Self.unique(url.lastPathComponent, in: dir)
                        if (try? FileManager.default.copyItem(at: url, to: dest)) != nil {
                            paths.append(dest.path)
                        }
                    }
                    await importStaged(paths, failed: urls.count - paths.count, model)
                } catch {
                    model.show(error.localizedDescription, error: true)
                }
            }
        }
    }

    /// A folder chosen in the Files picker: the pictures in it and its subfolders.
    func folder(_ result: Result<[URL], Error>, _ model: AppModel) {
        guard case .success(let urls) = result, let root = urls.first else {
            if case .failure(let e) = result { model.show(e.localizedDescription, error: true) }
            return
        }
        Task {
            do {
                let dir = try Self.batchDir()
                let scoped = root.startAccessingSecurityScopedResource()
                defer { if scoped { root.stopAccessingSecurityScopedResource() } }
                let files = Self.pictures(in: root)
                var paths: [String] = []
                for url in files {
                    let dest = Self.unique(url.lastPathComponent, in: dir)
                    if (try? FileManager.default.copyItem(at: url, to: dest)) != nil {
                        paths.append(dest.path)
                    }
                }
                if paths.isEmpty {
                    model.show(String(localized: "No photos in that folder"))
                    return
                }
                await importStaged(paths, failed: files.count - paths.count, model)
            } catch {
                model.show(error.localizedDescription, error: true)
            }
        }
    }

    /// Pictures dropped on the grid from another app (already copied into a batch folder).
    func dropped(_ files: [StagedFile], _ model: AppModel) {
        Task { await importStaged(files.map(\.url.path), failed: 0, model) }
    }

    /// The picture files under `root` (at most `maxFolderFiles`, `maxFolderDepth` levels down).
    nonisolated static func pictures(in root: URL) -> [URL] {
        guard let walk = FileManager.default.enumerator(at: root, includingPropertiesForKeys: [.contentTypeKey, .isRegularFileKey], options: [.skipsHiddenFiles]) else { return [] }
        var out: [URL] = []
        for case let url as URL in walk {
            if walk.level > maxFolderDepth {
                walk.skipDescendants()
                continue
            }
            let values = try? url.resourceValues(forKeys: [.contentTypeKey, .isRegularFileKey])
            guard values?.isRegularFile == true, let type = values?.contentType, type.conforms(to: .image) else { continue }
            out.append(url)
            if out.count >= maxFolderFiles { break }
        }
        return out
    }

    /// Review the staged files (`import.scan`), then the review moves the chosen ones into the
    /// library.
    private func importStaged(_ paths: [String], failed: Int, _ model: AppModel) async {
        if failed > 0 { model.show(String(localized: "\(failed) photos couldn't be read"), error: true) }
        guard !paths.isEmpty else { return }
        await model.reviewImport(paths)
    }
}

/// A file the Photos picker handed over (or another app dropped), in the system's own temporary folder.
nonisolated struct StagedFile: Transferable {
    let url: URL

    static var transferRepresentation: some TransferRepresentation {
        // a raw (ProRAW's DNG) before any image, as the egui host's picker asks
        FileRepresentation(importedContentType: .rawImage) { try stage($0.file) }
        FileRepresentation(importedContentType: .image) { try stage($0.file) }
    }

    /// The system's copy goes away when the import returns: keep our own.
    private static func stage(_ file: URL) throws -> StagedFile {
        let dir = try ImportModel.batchDir()
        let dest = ImportModel.unique(name(of: file), in: dir)
        try FileManager.default.copyItem(at: file, to: dest)
        return StagedFile(url: dest)
    }

    /// `file`'s name, with the extension of the format ImageIO finds in it when the name says another: the
    /// Photos picker handed an iPhone 17 Pro's ProRAW (a DNG) over as `IMG_8918.jpeg` (iOS 27.2), and the
    /// library, exports and sync keep the name.
    static func name(of file: URL) -> String {
        let name = file.lastPathComponent
        guard let source = CGImageSourceCreateWithURL(file as CFURL, nil),
              let id = CGImageSourceGetType(source) as String?,
              let found = UTType(id), let ext = found.preferredFilenameExtension,
              UTType(filenameExtension: file.pathExtension)?.conforms(to: found) != true
        else { return name }
        return (name as NSString).deletingPathExtension + "." + ext
    }
}
