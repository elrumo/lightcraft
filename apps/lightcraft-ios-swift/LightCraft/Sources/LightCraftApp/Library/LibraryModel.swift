#if os(iOS)
import Foundation
import LightCraftCore
import Observation

/// What the grid shows: the engine's current source, filter and sort, read a page at a time as the
/// grid scrolls (a library of 200 000 photos is never listed whole), with its date headers.
@Observable
final class LibraryModel {
    /// Photos in the grid (`state.visibleCount`).
    private(set) var count = 0
    /// Source, filter, sort, selection.
    private(set) var state: LibraryState?
    /// The grid's date headers (empty when the sort isn't by date).
    private(set) var groups: [DateGroup] = []
    /// The photos chosen (select mode, and the targets of the action bar).
    private(set) var selected: Set<UInt64> = []
    private(set) var stats: CatalogStats?
    private(set) var labels: [LabelName] = []
    /// The photo in the middle of the grid where it was left (written when scrolling comes to rest), for a grid the
    /// system makes again (a fold, an unfold, a rotation) to scroll back to. Another photo list clears it.
    var anchor: Int?

    private var pages: [Int: [PhotoSummary]] = [:]
    private var loading: Set<Int> = []
    /// Pages whose photos changed (a rating, an edit): kept (the loupe finds its photo in them) and
    /// read again when they are next shown.
    private var stale: Set<Int> = []
    /// Bumped when the photo list is another one: pages loading for the old list are dropped.
    private var generation = 0
    static let pageSize = 200

    /// The photo at `index` in the grid, loading its page when it isn't here yet.
    func photo(at index: Int, _ engine: Engine?) -> PhotoSummary? {
        guard index >= 0, index < count else { return nil }
        let page = index / Self.pageSize
        if let p = pages[page] {
            if stale.contains(page), let engine { load(page, engine) }
            let i = index - page * Self.pageSize
            return i < p.count ? p[i] : nil
        }
        if let engine { load(page, engine) }
        return nil
    }

    /// The photo at `index` if its page is here (no loading).
    func loaded(at index: Int) -> PhotoSummary? {
        let page = index / Self.pageSize
        guard let p = pages[page] else { return nil }
        let i = index - page * Self.pageSize
        return i >= 0 && i < p.count ? p[i] : nil
    }

    /// Where `id` is in the pages loaded so far.
    func index(of id: UInt64) -> Int? {
        for (page, photos) in pages {
            if let i = photos.firstIndex(where: { $0.id == id }) { return page * Self.pageSize + i }
        }
        return nil
    }

    private func load(_ page: Int, _ engine: Engine) {
        guard !loading.contains(page) else { return }
        loading.insert(page)
        let mine = generation
        Task {
            let params: JSONValue = ["offset": .int(Int64(page * Self.pageSize)), "limit": .int(Int64(Self.pageSize))]
            let result = try? await engine.execute("catalog.query", params, as: PhotoPage.self)
            guard mine == generation else { return }
            loading.remove(page)
            if let result {
                stale.remove(page)
                pages[page] = result.photos
                if result.total != count { count = result.total }
            }
        }
    }

    /// How the date headers group when the sort says `auto`: by month on a phone, by year when the
    /// cells are small (as the egui phone grid does).
    var autoGroup = "month"

    /// Another photo list (source, filter or sort changed; any catalog change, which renumbers the
    /// list too): start over. The pages here stay on show, stale, until they are read again, so the
    /// loupe's photo doesn't blink out at every edit; `forget` drops them (another library: photo
    /// ids mean something else).
    func reload(_ engine: Engine, count: Int, forget: Bool = false) async {
        generation += 1
        anchor = nil
        if forget { pages.removeAll() }
        loading.removeAll()
        stale = Set(pages.keys)
        self.count = count
        await refreshState(engine)
        await reloadGroups(engine)
        await refreshCounts(engine)
    }

    /// The date headers (the sort's grouping; `auto` follows the cells' size).
    func reloadGroups(_ engine: Engine) async {
        let group = state?.sort.group ?? "auto"
        let params: JSONValue = group == "auto" ? ["by": .string(autoGroup)] : [:]
        groups = (try? await engine.execute("library.groups", params, as: [DateGroup].self)) ?? []
    }

    /// The same list, changed photos (a rating, an edit): read the pages on screen again.
    func refresh(_ engine: Engine, keeping visible: Range<Int>?) async {
        let mine = generation
        let wanted = Set(visible.map { stride(from: $0.lowerBound / Self.pageSize, through: max($0.lowerBound, $0.upperBound - 1) / Self.pageSize, by: 1) }.map(Array.init) ?? [])
        // pages off screen are read again when they are next shown (kept meanwhile)
        stale.formUnion(pages.keys.filter { !wanted.contains($0) })
        for page in wanted.sorted() {
            let params: JSONValue = ["offset": .int(Int64(page * Self.pageSize)), "limit": .int(Int64(Self.pageSize))]
            guard let result = try? await engine.execute("catalog.query", params, as: PhotoPage.self), mine == generation else { continue }
            stale.remove(page)
            pages[page] = result.photos
            count = result.total
        }
        await refreshCounts(engine)
    }

    /// Source, filter, sort and the selection.
    func refreshState(_ engine: Engine) async {
        guard let s = try? await engine.execute("library.state", [:], as: LibraryState.self) else { return }
        state = s
        selected = Set(s.selection.ids)
    }

    /// The albums list's counts and the label names.
    func refreshCounts(_ engine: Engine) async {
        stats = try? await engine.execute("catalog.stats", [:], as: CatalogStats.self)
        if labels.isEmpty {
            labels = (try? await engine.execute("label.names", [:], as: [LabelName].self)) ?? []
        }
    }

    func labelName(_ label: String) -> String {
        labels.first { $0.label == label }?.name ?? label.capitalized
    }

    /// The header the photo at `index` is under.
    func group(at index: Int) -> DateGroup? {
        groups.last { $0.start <= index }
    }
}
#endif
