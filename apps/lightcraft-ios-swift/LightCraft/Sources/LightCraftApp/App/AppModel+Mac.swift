#if os(macOS)
import AppKit
import LightCraftCore

/// What only a Mac's app model does: quitting well, and pictures that arrive from Finder.
extension AppModel {
    /// What quitting now would cost, in words, or nil: a task still running (an export, an import, a
    /// sync) or changes that are not on disk yet.
    var quitWarning: String? {
        let running = activity.filter { !$0.cancelling }
        if let first = running.first {
            return running.count == 1
                ? String(localized: "“\(first.label)” is still running.")
                : String(localized: "\(running.count) tasks are still running.")
        }
        return state.unsaved > 0 ? String(localized: "Some changes are not saved yet.") : nil
    }

    /// Save, close the library (the egui app may open it now) and stop the engine: what Quit does.
    func terminate() async {
        guard let engine else { return }
        _ = try? await engine.request("host.lifecycle", ["event": "terminate"])
        await engine.close()
    }

    /// File ▸ Import Photos…: the open panel, for pictures and for folders of them, which are copied
    /// into a staging folder and reviewed as the photos picker's are (the Mac's own import, in place
    /// and with a mode, is SM3).
    func importPhotosPanel() {
        let panel = NSOpenPanel()
        panel.title = String(localized: "Import Photos")
        panel.canChooseFiles = true
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = true
        panel.allowedContentTypes = ImportModel.fileTypes
        guard panel.runModal() == .OK else { return }
        importFromFinder(panel.urls)
    }

    /// Pictures opened from Finder or dropped on the Dock icon: copied into a staging folder and
    /// reviewed, as the photos picker's are (the Mac's own import, in place, is SM3).
    func importFromFinder(_ urls: [URL]) {
        guard engine != nil else { return }
        // (a folder brings its pictures, its subfolders' too)
        let files = urls.flatMap { url in
            (try? url.resourceValues(forKeys: [.isDirectoryKey]))?.isDirectory == true ? ImportModel.pictures(in: url) : [url]
        }
        guard !files.isEmpty else {
            show(String(localized: "No photos found"))
            return
        }
        ImportModel().files(.success(files), self)
    }
}
#endif
