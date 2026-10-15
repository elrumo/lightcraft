import LightCraftCore
import SwiftUI

/// Albums ▸ By Date: year → month → day with photo counts (`catalog.stats` → `byDate`); a tap
/// shows those photos (from All Photos, filtered by that date).
struct ByDateView: View {
    @Environment(AppModel.self) private var model
    @State private var years: [JSONValue] = []

    var body: some View {
        List {
            ForEach(Array(years.enumerated()), id: \.offset) { _, y in
                let year = y["year"].string ?? ""
                DisclosureGroup {
                    ForEach(pairs(y["months"]), id: \.0) { month, n in
                        DisclosureGroup {
                            ForEach(pairs(y["days"]).filter { $0.0.hasPrefix(month) }, id: \.0) { day, n in
                                row(day, n)
                            }
                        } label: {
                            row(month, n)
                        }
                    }
                } label: {
                    row(year, Int(y["count"].int ?? 0))
                }
            }
            if years.isEmpty {
                Text("No dated photos").foregroundStyle(.secondary)
            }
        }
        .navigationTitle("By Date")
        .inlineTitle()
        .task(id: model.settledRevision) {
            guard let engine = model.engine else { return }
            years = (try? await engine.execute("catalog.stats"))?["byDate"].array ?? []
        }
    }

    private func pairs(_ v: JSONValue) -> [(String, Int)] {
        (v.array ?? []).compactMap { p in
            guard let key = p.array?[safe: 0]?.string else { return nil }
            return (key, Int(p.array?[safe: 1]?.int ?? 0))
        }
    }

    private func row(_ key: String, _ n: Int) -> some View {
        Button {
            Task {
                await model.run("library.source", ["kind": "all"])
                await model.run("library.filter", ["date": .string(key)])
                model.sheet = nil
            }
        } label: {
            LabeledContent(Self.label(key)) { Text(verbatim: "\(n)").monospacedDigit() }
        }
        .foregroundStyle(.primary)
    }

    /// `2026`, `May 2026`, `14 May 2026` (in the device's language).
    static func label(_ key: String) -> String { dateTitle(key: key, label: key, weekday: false) }
}

/// A date header in the reader's language: a date key (`2026`, `2026-05`, `2026-05-14`, with its
/// weekday when `weekday`) formatted here; anything else (a file-name group, No Date) as the engine
/// names it, translated.
func dateTitle(key: String, label: String, weekday: Bool = true, abbreviated: Bool = false) -> String {
    let parts = key.split(separator: "-", omittingEmptySubsequences: false).map { Int($0) }
    guard (1...3).contains(parts.count), parts.allSatisfy({ $0 != nil }), key.count == [4, 7, 10][parts.count - 1] else {
        return Bundle.main.localizedString(forKey: label, value: label, table: nil)
    }
    var c = DateComponents()
    c.year = parts[0]
    c.month = parts.count > 1 ? parts[1] : nil
    c.day = parts.count > 2 ? parts[2] : nil
    guard let date = Calendar.current.date(from: c) else { return label }
    let style: Date.FormatStyle = switch parts.count {
    case 1: .dateTime.year()
    case 2: .dateTime.year().month(abbreviated ? .abbreviated : .wide)
    default: weekday ? .dateTime.weekday(.wide).year().month(.wide).day() : .dateTime.year().month(.wide).day()
    }
    return date.formatted(style)
}

/// Albums ▸ Folders: the disks and folders the library's photos were imported from, with photo
/// counts (`library.folders`); a tap shows a folder's photos (`library.source {libraryFolder}`).
struct LibraryFoldersView: View {
    @Environment(AppModel.self) private var model
    @State private var removing: FolderNode?
    @State private var tree: [FolderNode] = []

    var body: some View {
        List {
            OutlineGroup(tree, children: \.nested) { node in
                Button {
                    guard node.selectable else { return }
                    Task {
                        await model.run("library.source", ["kind": "libraryFolder", "path": .string(node.path)])
                        model.sheet = nil
                    }
                } label: {
                    LabeledContent {
                        Text(verbatim: "\(node.count)").monospacedDigit()
                    } label: {
                        Label(node.name, systemImage: node.volume ? "externaldrive" : "folder")
                    }
                }
                .foregroundStyle(.primary)
                .contextMenu {
                    Button(node.volume ? "Remove Disk from Library…" : "Remove Folder from Library…", systemImage: "folder.badge.minus", role: .destructive) {
                        removing = node
                    }
                }
            }
            if tree.isEmpty {
                Text("No folders yet: photos imported from Files show their folders here.").foregroundStyle(.secondary)
            }
        }
        .navigationTitle("Folders")
        .inlineTitle()
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button("Find Missing Photos…", systemImage: "magnifyingglass") { model.picker = .findMissing }
            }
        }
        .confirmationDialog(removing.map { String(localized: "Remove “\($0.name)” from the library?") } ?? "",
                            isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }), titleVisibility: .visible) {
            Button("Remove", role: .destructive) {
                if let f = removing { model.perform("library.removeFolder", ["path": .string(f.path), "disk": .bool(f.volume)]) }
            }
        } message: {
            Text("Its photos go to Recently Deleted; no file is touched.")
        }
        .task(id: model.settledRevision) {
            guard let engine = model.engine else { return }
            tree = (try? await engine.execute("library.folders", [:], as: [FolderNode].self)) ?? []
        }
    }
}

/// A folder of `library.folders`.
nonisolated struct FolderNode: Decodable, Identifiable, Hashable {
    var name: String
    var path: String
    var count: Int
    var volume: Bool
    var selectable: Bool
    var children: [FolderNode]
    var id: String { path }
    var nested: [FolderNode]? { children.isEmpty ? nil : children }

    enum CodingKeys: String, CodingKey { case name, path, count, volume, selectable, children }

    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: CodingKeys.self)
        name = try c.decode(String.self, forKey: .name)
        path = try c.decode(String.self, forKey: .path)
        count = (try? c.decode(Int.self, forKey: .count)) ?? 0
        volume = (try? c.decode(Bool.self, forKey: .volume)) ?? false
        selectable = (try? c.decode(Bool.self, forKey: .selectable)) ?? true
        children = (try? c.decode([FolderNode].self, forKey: .children)) ?? []
    }
}
