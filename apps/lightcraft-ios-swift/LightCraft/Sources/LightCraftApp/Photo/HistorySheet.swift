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
            LCList {
                if versions {
                    versionsList
                } else {
                    historyList
                }
            }
            .foregroundStyle(.primary)
            .navigationTitle(versions ? "Versions" : "History")
            .inlineTitle()
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

    /// Every edit, newest first: one to go back to it.
    @ViewBuilder private var historyList: some View {
        let history = list?.history ?? []
        if history.isEmpty {
            Section {
                LCEmpty(title: "No Edits Yet", symbol: "clock.arrow.circlepath", message: "Each change made to this photo is listed here, to go back to.")
            }
        } else {
            Section {
                ForEach(Array(history.enumerated().reversed()), id: \.offset) { i, label in
                    Button {
                        model.perform("history.restore", ["index": .int(Int64(i))])
                    } label: {
                        Label(label, systemImage: "clock")
                    }
                    .lcRowButton()
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
            } header: {
                Text("Newest First")
            } footer: {
                Text("Choose a step to go back to how the photo was then; right-click one for more.")
            }
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

    /// The looks kept for this photo: create, restore, rename, update, delete.
    @ViewBuilder private var versionsList: some View {
        let kept = list?.versions ?? []
        Section {
            Button {
                Task {
                    await model.run("version.create")
                    await load()
                }
            } label: {
                Label("Create Version", systemImage: "plus.circle.fill")
            }
            .lcRowButton()
        }
        if kept.isEmpty {
            Section {
                LCEmpty(title: "No Versions", symbol: "square.stack", message: "Versions keep a look of this photo to come back to.")
            }
        } else {
            Section {
                ForEach(Array(kept.enumerated()), id: \.offset) { i, v in
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
                    .lcRowButton()
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
            } header: {
                Text("Versions")
            } footer: {
                Text("Choose a version to bring it back; right-click one to rename, update or delete it.")
            }
        }
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
            LCList {
                Section {
                    HStack {
                        TextField("Add Keyword", text: $typed)
                            .noAutocap()
                            .onSubmit { add(typed) }
                        Button("Add") { add(typed) }.disabled(typed.trimmingCharacters(in: .whitespaces).isEmpty)
                    }
                    ForEach(suggestions, id: \.self) { s in
                        Button { add(s) } label: { Label(s, systemImage: "plus.circle") }
                            .lcRowButton()
                    }
                } footer: {
                    Text("Type a keyword and press Return; the library's keywords come up as you type.")
                }
                Section("On This Photo") {
                    if keywords.isEmpty {
                        LCEmpty(title: "No Keywords", symbol: "tag", message: "Add one above.")
                    }
                    ForEach(keywords, id: \.self) { k in
                        HStack {
                            Label(k, systemImage: "tag")
                            #if os(macOS)
                            Spacer()
                            Button("Remove", systemImage: "minus.circle.fill") { remove(k) }
                                .labelStyle(.iconOnly)
                                .buttonStyle(.borderless)
                                .foregroundStyle(.secondary)
                                .help(Text("Remove from this photo"))
                            #endif
                        }
                        .swipeActions {
                            Button("Remove", role: .destructive) { remove(k) }
                        }
                    }
                }
            }
            .navigationTitle("Keywords")
            .inlineTitle()
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
