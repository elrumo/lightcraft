#if os(iOS)
import Foundation
import LightCraftCore
import Observation

/// The albums tree (`albums.list`) and the server sync's status, read again when the catalog
/// changes (at most every second or so: counts don't need to follow every keystroke).
@Observable
final class AlbumsModel {
    private(set) var roots: [AlbumNode] = []
    private(set) var sync: SyncStatus?
    private var pending = false

    func reload(_ engine: Engine) async {
        roots = (try? await engine.execute("albums.list", [:], as: [AlbumNode].self)) ?? roots
        sync = try? await engine.execute("sync.status", [:], as: SyncStatus.self)
    }

    func scheduleReload(_ engine: Engine) {
        guard !pending else { return }
        pending = true
        Task {
            try? await Task.sleep(for: .milliseconds(800))
            pending = false
            await reload(engine)
        }
    }

    func node(_ id: UInt64) -> AlbumNode? {
        roots.lazy.flatMap { $0.flattened() }.first { $0.id == id }
    }

    /// Plain albums (photos can be put in them by hand), A–Z.
    var plain: [AlbumNode] {
        roots.flatMap { $0.flattened() }.filter(\.isPlain).sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
    }

    /// Folders (an album can go in one), A–Z.
    var folders: [AlbumNode] {
        roots.flatMap { $0.flattened() }.filter(\.folder).sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
    }

    /// The album is kept on this device (a synced library).
    func isOffline(_ id: UInt64) -> Bool {
        sync?.offlineAlbums?.contains(id) == true
    }
}
#endif
