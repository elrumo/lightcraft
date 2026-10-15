import UIKit
import UniformTypeIdentifiers

/// "Add to LightCraft" in the system share sheet: copies the pictures shared into the app group's
/// `Inbox/<batch>` folder; the app takes them into its import review the next time it is active
/// (`SharedInbox.swift` in the app; keep `group` the same in both). No engine runs in here.
final class ShareViewController: UIViewController {
    nonisolated static let group = "group.ai.storyteller.lightcraft.swift"

    private let label = UILabel()

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground
        label.text = NSLocalizedString("Adding to LightCraft…", comment: "")
        label.font = .preferredFont(forTextStyle: .headline)
        label.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(label)
        NSLayoutConstraint.activate([
            label.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            label.centerYAnchor.constraint(equalTo: view.centerYAnchor),
        ])
        let providers = (extensionContext?.inputItems as? [NSExtensionItem])?.flatMap { $0.attachments ?? [] } ?? []
        Task {
            let n = await Self.save(providers)
            label.text = n > 0
                ? String(format: NSLocalizedString("Added %d to LightCraft. Open the app to import.", comment: ""), n)
                : NSLocalizedString("LightCraft couldn't read that.", comment: "")
            try? await Task.sleep(for: .seconds(n > 0 ? 1.2 : 2))
            extensionContext?.completeRequest(returningItems: nil)
        }
    }

    /// Copy each picture into a new batch folder of the app group; how many were saved.
    private static func save(_ providers: [NSItemProvider]) async -> Int {
        guard let root = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group) else { return 0 }
        let batch = root.appending(path: "Inbox").appending(path: UUID().uuidString)
        guard (try? FileManager.default.createDirectory(at: batch, withIntermediateDirectories: true)) != nil else { return 0 }
        var saved = 0
        for provider in providers {
            // a raw or DNG (.rawImage) before the generic image, as the app's picker asks
            guard let type = [UTType.rawImage, .image].first(where: { provider.hasItemConformingToTypeIdentifier($0.identifier) }) else { continue }
            let n = saved
            let url: URL? = await withCheckedContinuation { done in
                _ = provider.loadFileRepresentation(forTypeIdentifier: type.identifier) { file, _ in
                    // (the system's copy goes away when this returns: copy now)
                    guard let file else { return done.resume(returning: nil) }
                    let dest = batch.appending(path: "\(n)-\(file.lastPathComponent)")
                    done.resume(returning: (try? FileManager.default.copyItem(at: file, to: dest)) == nil ? nil : dest)
                }
            }
            if url != nil { saved += 1 }
        }
        if saved == 0 { try? FileManager.default.removeItem(at: batch) }
        return saved
    }
}
