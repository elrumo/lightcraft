import Foundation

#if os(iOS)
extension AppModel {
    /// What the "Add to LightCraft" share extension left in the app group's `Inbox` (ShareViewController.swift):
    /// moved into the staging area, then opened in the import review. Run when the app becomes active.
    func importShared() async {
        guard engine != nil, let root = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: "group.ai.storyteller.lightcraft.swift") else { return }
        let inbox = root.appending(path: "Inbox")
        let fm = FileManager.default
        var paths: [String] = []
        for batch in (try? fm.contentsOfDirectory(at: inbox, includingPropertiesForKeys: nil)) ?? [] {
            guard let dir = try? ImportModel.batchDir() else { continue }
            for file in (try? fm.contentsOfDirectory(at: batch, includingPropertiesForKeys: nil)) ?? [] {
                let dest = ImportModel.unique(file.lastPathComponent, in: dir)
                if (try? fm.moveItem(at: file, to: dest)) != nil { paths.append(dest.path) }
            }
            try? fm.removeItem(at: batch)
        }
        if !paths.isEmpty { await reviewImport(paths) }
    }
}
#endif
