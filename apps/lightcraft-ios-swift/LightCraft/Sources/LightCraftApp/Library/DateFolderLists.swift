#if os(iOS)
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
        .navigationBarTitleDisplayMode(.inline)
        .task(id: model.state.catalogRevision) {
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
    static func label(_ key: String) -> String {
        let parts = key.split(separator: "-").compactMap { Int($0) }
        var c = DateComponents()
        c.year = parts.first
        c.month = parts.count > 1 ? parts[1] : nil
        c.day = parts.count > 2 ? parts[2] : nil
        guard let date = Calendar.current.date(from: c) else { return key }
        let style: Date.FormatStyle = switch parts.count {
        case 1: .dateTime.year()
        case 2: .dateTime.year().month(.wide)
        default: .dateTime.year().month(.wide).day()
        }
        return date.formatted(style)
    }
}

/// Albums ▸ Folders: the disks and folders the library's photos were imported from, with photo
/// counts (`library.folders`); a tap shows a folder's photos (`library.source {libraryFolder}`).
struct LibraryFoldersView: View {
    @Environment(AppModel.self) private var model
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
            }
            if tree.isEmpty {
                Text("No folders yet: photos imported from Files show their folders here.").foregroundStyle(.secondary)
            }
        }
        .navigationTitle("Folders")
        .navigationBarTitleDisplayMode(.inline)
        .task(id: model.state.catalogRevision) {
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
#endif
