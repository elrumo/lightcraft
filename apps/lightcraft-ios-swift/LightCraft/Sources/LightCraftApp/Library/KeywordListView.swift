#if os(iOS)
import LightCraftCore
import SwiftUI

/// The library's keywords (`keyword.list`: `a|b|c` keywords nest), each with its photo count. A
/// tap shows its photos in the grid; a long press renames it, edits its tag (synonyms, export,
/// person), makes a keyword inside it, or deletes it from every photo. + creates a keyword tag.
struct KeywordListView: View {
    @Environment(AppModel.self) private var model
    @State private var tree: [KeywordNode] = []
    @State private var renaming: KeywordNode?
    @State private var newName = ""
    @State private var deleting: KeywordNode?
    @State private var tag: KeywordTagTarget?

    var body: some View {
        List {
            OutlineGroup(tree, children: \.nested) { node in
                Button {
                    model.perform("library.filter", ["keyword": .string(node.path)])
                    model.sheet = nil
                } label: {
                    LabeledContent(node.name) { Text(verbatim: "\(node.count)").monospacedDigit() }
                }
                .foregroundStyle(.primary)
                .contextMenu {
                    Button("Rename…", systemImage: "pencil") {
                        newName = node.name
                        renaming = node
                    }
                    Button("Edit Keyword Tag…", systemImage: "tag") { tag = KeywordTagTarget(editing: node.path, parent: nil) }
                    Button("Create Keyword Inside…", systemImage: "plus") { tag = KeywordTagTarget(editing: nil, parent: node.path) }
                    Divider()
                    Button("Delete…", systemImage: "trash", role: .destructive) { deleting = node }
                }
            }
            if tree.isEmpty {
                Text("No keywords yet").foregroundStyle(.secondary)
            }
        }
        .navigationTitle("Keywords")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button("Create Keyword Tag", systemImage: "plus") { tag = KeywordTagTarget(editing: nil, parent: nil) }
            }
        }
        .alert("Rename Keyword", isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
            TextField("Keyword", text: $newName)
            Button("Rename") { rename() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("On every photo, the keywords inside it included; renaming it onto another keyword merges the two.")
        }
        .confirmationDialog(deleting.map { String(localized: "Delete “\($0.name)” from \($0.count) photos?") } ?? "",
                            isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }), titleVisibility: .visible) {
            Button("Delete Keyword", role: .destructive) {
                if let k = deleting { model.perform("keyword.delete", ["keyword": .string(k.path)]) }
            }
        }
        .sheet(item: $tag) { KeywordTagSheet(target: $0) }
        .task(id: model.state.catalogRevision) {
            guard let engine = model.engine else { return }
            tree = (try? await engine.execute("keyword.list", [:], as: [KeywordNode].self)) ?? []
        }
    }

    /// The new name replaces the last level of the keyword's path.
    private func rename() {
        guard let k = renaming else { return }
        let name = newName.trimmingCharacters(in: .whitespaces)
        guard !name.isEmpty, name != k.name else { return }
        var parts = k.path.split(separator: "|").map(String.init)
        if !parts.isEmpty { parts[parts.count - 1] = name }
        model.perform("keyword.rename", ["from": .string(k.path), "to": .string(parts.joined(separator: "|"))])
    }
}

/// A keyword of `keyword.list`.
nonisolated struct KeywordNode: Decodable, Identifiable, Hashable {
    var name: String
    var path: String
    var count: Int
    var children: [KeywordNode]
    var id: String { path }
    /// Its keywords, or `nil` when it has none (a leaf).
    var nested: [KeywordNode]? { children.isEmpty ? nil : children }
}

/// Create Keyword Tag (inside `parent`) or Edit Keyword Tag (`editing`).
struct KeywordTagTarget: Identifiable {
    let id = UUID()
    let editing: String?
    let parent: String?
}

/// Create or Edit Keyword Tag: its name, synonyms, whether it (and the keywords it is in, and its
/// synonyms) go out with exports, whether it names a person; a new one can go on the selected photos.
struct KeywordTagSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let target: KeywordTagTarget
    @State private var name = ""
    @State private var synonyms = ""
    @State private var includeOnExport = true
    @State private var exportContaining = true
    @State private var exportSynonyms = true
    @State private var person = false
    @State private var addToSelected = false

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Keyword Name", text: $name)
                        .textInputAutocapitalization(.never)
                    TextField("Synonyms (separated by commas)", text: $synonyms)
                        .textInputAutocapitalization(.never)
                } footer: {
                    if let parent = target.parent { Text("Inside “\(parent)”") }
                }
                Section("Keyword Tag Options") {
                    Toggle("Include on Export", isOn: $includeOnExport)
                    Toggle("Export Containing Keywords", isOn: $exportContaining)
                    Toggle("Export Synonyms", isOn: $exportSynonyms)
                    Toggle("Person", isOn: $person)
                }
                if target.editing == nil {
                    Section {
                        Toggle("Add to Selected Photos", isOn: $addToSelected)
                    }
                }
            }
            .navigationTitle(target.editing == nil ? "Create Keyword Tag" : "Edit Keyword Tag")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel", systemImage: "xmark") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button(target.editing == nil ? "Create" : "Save") { save() }
                        .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            }
        }
        .task {
            guard let editing = target.editing, let engine = model.engine, let info = try? await engine.execute("keyword.info", ["keyword": .string(editing)]) else { return }
            name = editing.split(separator: "|").last.map(String.init) ?? editing
            synonyms = info["synonyms"].string ?? (info["synonyms"].array ?? []).compactMap(\.string).joined(separator: ", ")
            includeOnExport = info["includeOnExport"].bool ?? true
            exportContaining = info["exportContaining"].bool ?? true
            exportSynonyms = info["exportSynonyms"].bool ?? true
            person = info["person"].bool ?? false
        }
    }

    private func save() {
        let syn = JSONValue.array(synonyms.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }.map { .string($0) })
        var p: [String: JSONValue] = [
            "name": .string(name.trimmingCharacters(in: .whitespaces)), "synonyms": syn, "includeOnExport": .bool(includeOnExport),
            "exportContaining": .bool(exportContaining), "exportSynonyms": .bool(exportSynonyms), "person": .bool(person),
        ]
        Task {
            let done: JSONValue?
            if let editing = target.editing {
                p["keyword"] = .string(editing)
                done = await model.run("keyword.edit", .object(p))
            } else {
                p["parent"] = target.parent.map { .string($0) } ?? .null
                p["addToSelected"] = .bool(addToSelected)
                done = await model.run("keyword.create", .object(p))
            }
            if done != nil { dismiss() }
        }
    }
}
#endif
