#if os(iOS)
import LightCraftCore
import SwiftUI

/// The photo's History (every edit, newest first: tap one to go back to it) and Versions (named
/// looks to keep: create, restore, rename, update, delete).
struct HistorySheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State var versions: Bool
    @State private var list: HistoryList?
    @State private var naming: Int?
    @State private var name = ""

    var body: some View {
        NavigationStack {
            List {
                if versions {
                    Section {
                        Button("Create Version", systemImage: "plus") {
                            Task {
                                await model.run("version.create")
                                await load()
                            }
                        }
                    }
                    Section {
                        ForEach(Array((list?.versions ?? []).enumerated()), id: \.offset) { i, v in
                            Button {
                                model.perform("version.restore", ["index": .int(Int64(i))])
                                dismiss()
                            } label: {
                                HStack(spacing: 12) {
                                    VariantPreview(which: ["version": .int(Int64(i))])
                                        .frame(width: 64, height: 44)
                                        .clipShape(.rect(cornerRadius: 6))
                                    VStack(alignment: .leading) {
                                        Text(v.name)
                                        if let created = v.created {
                                            Text(InfoPanel.date(created)).font(.caption).foregroundStyle(.secondary)
                                        }
                                    }
                                }
                            }
                            .contextMenu {
                                Button("Update with Current Settings", systemImage: "arrow.triangle.2.circlepath") { run("version.update", i) }
                                Button("Rename…", systemImage: "pencil") {
                                    name = v.name
                                    naming = i
                                }
                                Button("Set as Before", systemImage: "square.split.2x1") {
                                    model.perform("beforeAfter.setBefore", ["source": "version", "name": .string(v.name)])
                                }
                                Button("Delete", systemImage: "trash", role: .destructive) { run("version.delete", i) }
                            }
                        }
                    } footer: {
                        if list?.versions.isEmpty == true { Text("Versions keep a look of this photo to come back to.") }
                    }
                } else {
                    Section {
                        let history = list?.history ?? []
                        ForEach(Array(history.enumerated().reversed()), id: \.offset) { i, label in
                            Button {
                                model.perform("history.restore", ["index": .int(Int64(i))])
                            } label: {
                                Label(label, systemImage: "clock")
                            }
                            .contextMenu {
                                Button("Copy to Before", systemImage: "square.split.2x1") {
                                    model.perform("beforeAfter.setBefore", ["source": "history", "index": .int(Int64(i))])
                                }
                                Button("Create Version from Step", systemImage: "square.stack") {
                                    Task {
                                        await model.run("history.restore", ["index": .int(Int64(i))])
                                        await model.run("version.create", ["name": .string(label)])
                                    }
                                }
                            }
                        }
                    } footer: {
                        if list?.history.isEmpty == true { Text("No edits yet.") }
                    }
                    if list?.history.isEmpty == false {
                        Section {
                            Button("Clear History", role: .destructive) {
                                Task {
                                    await model.run("history.clear")
                                    await load()
                                }
                            }
                        }
                    }
                }
            }
            .foregroundStyle(.primary)
            .navigationTitle(versions ? "Versions" : "History")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .principal) {
                    Picker("Show", selection: $versions) {
                        Text("History").tag(false)
                        Text("Versions").tag(true)
                    }
                    .pickerStyle(.segmented)
                    .frame(width: 220)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done", systemImage: "checkmark") { dismiss() }
                }
            }
            .task(id: model.settledRevision) { await load() }
            .alert("Rename Version", isPresented: Binding(get: { naming != nil }, set: { if !$0 { naming = nil } })) {
                TextField("Name", text: $name)
                Button("Rename") {
                    if let i = naming { model.perform("version.rename", ["index": .int(Int64(i)), "name": .string(name)]) }
                    naming = nil
                }
                Button("Cancel", role: .cancel) { naming = nil }
            }
        }
        .presentationDetents([.medium, .large])
    }

    private func run(_ command: String, _ index: Int) {
        Task {
            await model.run(command, ["index": .int(Int64(index))])
            await load()
        }
    }

    private func load() async {
        guard let engine = model.engine else { return }
        list = try? await engine.execute("history.list", [:], as: HistoryList.self)
    }
}

/// The photo's keywords: remove one, add one (suggestions as you type), or pick from the library's.
struct KeywordsSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var keywords: [String] = []
    @State private var typed = ""
    @State private var suggestions: [String] = []

    var body: some View {
        NavigationStack {
            List {
                Section {
                    HStack {
                        TextField("Add Keyword", text: $typed)
                            .textInputAutocapitalization(.never)
                            .onSubmit { add(typed) }
                        Button("Add") { add(typed) }.disabled(typed.trimmingCharacters(in: .whitespaces).isEmpty)
                    }
                    ForEach(suggestions, id: \.self) { s in
                        Button(s) { add(s) }
                    }
                }
                Section("On This Photo") {
                    ForEach(keywords, id: \.self) { k in
                        Text(k)
                            .swipeActions {
                                Button("Remove", role: .destructive) { remove(k) }
                            }
                    }
                    if keywords.isEmpty { Text("No keywords").foregroundStyle(.secondary) }
                }
            }
            .navigationTitle("Keywords")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done", systemImage: "checkmark") { dismiss() }
                }
            }
            .task(id: model.settledRevision) { await load() }
            .task(id: typed) {
                guard let engine = model.engine else { return }
                let r = try? await engine.execute("keyword.suggest", ["prefix": .string(typed), "limit": 8])
                suggestions = (r?.array?.compactMap(\.string) ?? []).filter { !keywords.contains($0) }
            }
        }
        .presentationDetents([.medium, .large])
    }

    private var ids: JSONValue { model.state.active.map { [.int(Int64(clamping: $0))] } ?? [] }

    private func add(_ k: String) {
        let k = k.trimmingCharacters(in: .whitespaces)
        guard !k.isEmpty else { return }
        typed = ""
        model.perform("photo.setMeta", ["ids": ids, "addKeywords": [.string(k)]])
    }

    private func remove(_ k: String) {
        model.perform("photo.setMeta", ["ids": ids, "removeKeywords": [.string(k)]])
    }

    private func load() async {
        guard let engine = model.engine, let r = try? await engine.execute("photo.inspect") else { return }
        keywords = r["meta"]["keywords"].array?.compactMap(\.string) ?? []
    }
}
#endif
