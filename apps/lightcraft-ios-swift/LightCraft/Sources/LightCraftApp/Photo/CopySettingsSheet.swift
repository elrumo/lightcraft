#if os(iOS)
import LightCraftCore
import SwiftUI

/// Choose the edit settings to copy from the photo, or to paste onto the photos chosen: a checklist
/// of the settings groups (`develop.copy {groups}`, `develop.paste {groups}`).
struct CopySettingsSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    /// Paste onto these photos; `nil`: copy from the photo.
    let pasteOnto: [UInt64]?
    @State private var groups = SettingsGroups()

    var body: some View {
        NavigationStack {
            List {
                SettingsGroupsSection(groups: $groups)
            }
            .navigationTitle(pasteOnto == nil ? "Copy Settings" : "Paste Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel", systemImage: "xmark") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button(pasteOnto == nil ? "Copy" : "Paste") {
                        Task {
                            if let ids = pasteOnto {
                                await model.run("develop.paste", ["ids": AppModel.ids(ids), "groups": groups.chosenJSON])
                            } else if await model.run("develop.copy", ["groups": groups.chosenJSON]) != nil {
                                model.show(String(localized: "Edit settings copied"))
                            }
                            dismiss()
                        }
                    }
                    .disabled(groups.chosen.isEmpty)
                }
            }
        }
    }
}

/// The settings groups (`develop.settingsGroups`: Profile, White Balance, Light…) and which are
/// ticked: at first the ones copied by default (not crop, masks, Remove, red eye).
struct SettingsGroups {
    var all: [(id: String, label: String)] = []
    var chosen: Set<String> = []

    /// The ticked ones, in the engine's order, as `develop.copy` / `preset.create` take them.
    var chosenJSON: JSONValue {
        .array(all.map(\.id).filter(chosen.contains).map { .string($0) })
    }
}

/// A checklist of the settings groups with All and None.
struct SettingsGroupsSection: View {
    @Environment(AppModel.self) private var model
    @Binding var groups: SettingsGroups

    var body: some View {
        Section {
            ForEach(groups.all, id: \.id) { g in
                Toggle(LocalizedStringKey(g.label), isOn: Binding(get: { groups.chosen.contains(g.id) }, set: { on in
                    if on { groups.chosen.insert(g.id) } else { groups.chosen.remove(g.id) }
                }))
            }
        } header: {
            HStack {
                Button("All") { groups.chosen = Set(groups.all.map(\.id)) }
                Spacer()
                Button("None") { groups.chosen = [] }
            }
            .font(.subheadline)
            .textCase(nil)
        }
        .task {
            guard groups.all.isEmpty, let engine = model.engine, let list = (try? await engine.execute("develop.settingsGroups"))?.array else { return }
            groups.all = list.compactMap { g in g["id"].string.map { ($0, g["label"].string ?? $0) } }
            groups.chosen = Set(list.filter { $0["copiedByDefault"].bool == true }.compactMap { $0["id"].string })
        }
    }
}

/// Create Preset: its name, its group (an existing one of yours, or a new one) and the settings
/// it keeps (`preset.create`).
struct CreatePresetSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var group = "User Presets"
    @State private var newGroup = ""
    @State private var userGroups: [String] = []
    @State private var groups = SettingsGroups()

    private static let new = "\u{0}new"

    var body: some View {
        NavigationStack {
            List {
                Section {
                    TextField("Preset Name", text: $name)
                    Picker("Group", selection: $group) {
                        ForEach(userGroups, id: \.self) { Text($0).tag($0) }
                        Text("New Group…").tag(Self.new)
                    }
                    if group == Self.new {
                        TextField("Group Name", text: $newGroup)
                    }
                }
                SettingsGroupsSection(groups: $groups)
            }
            .navigationTitle("Create Preset")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel", systemImage: "xmark") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Create") {
                        let g = group == Self.new ? newGroup.trimmingCharacters(in: .whitespaces) : group
                        Task {
                            if await model.run("preset.create", ["name": .string(name.trimmingCharacters(in: .whitespaces)), "group": .string(g.isEmpty ? "User Presets" : g), "groups": groups.chosenJSON]) != nil {
                                model.show(String(localized: "Preset created"))
                                dismiss()
                            }
                        }
                    }
                    .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty || groups.chosen.isEmpty)
                }
            }
        }
        .task {
            guard let engine = model.engine else { return }
            let presets = (try? await engine.execute("presets.list", [:], as: [PresetInfo].self)) ?? []
            var seen: [String] = ["User Presets"]
            for p in presets where !p.builtin && !seen.contains(p.group) {
                seen.append(p.group)
            }
            userGroups = seen
        }
    }
}
#endif
