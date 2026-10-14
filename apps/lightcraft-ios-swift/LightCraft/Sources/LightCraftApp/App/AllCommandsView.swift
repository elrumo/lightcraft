import LightCraftCore
import SwiftUI

/// Every command the engine has, grouped by menu, searchable, and runnable: what has no screen of
/// its own yet is still one tap (or one parameter form) away. A disabled command says why.
struct AllCommandsView: View {
    @Environment(AppModel.self) private var model
    @State private var commands: [CommandInfo] = []
    @State private var search = ""
    @State private var asking: CommandInfo?
    @State private var result: String?

    var body: some View {
        List {
            ForEach(groups, id: \.name) { group in
                Section(group.name) {
                    ForEach(group.commands) { command in
                        Button {
                            if command.takesNothing {
                                Task { await run(command, [:]) }
                            } else {
                                asking = command
                            }
                        } label: {
                            CommandRow(command: command)
                        }
                        .disabled(!command.enabled)
                    }
                }
            }
        }
        .searchable(text: $search, prompt: "Search commands")
        .navigationTitle("All Commands")
        .task(id: model.state) { await load() }
        .sheet(item: $asking) { command in
            CommandForm(command: command) { params in
                asking = nil
                Task { await run(command, params) }
            }
        }
        .alert("Done", isPresented: Binding(get: { result != nil }, set: { if !$0 { result = nil } })) {
            Button("OK") { result = nil }
        } message: {
            Text(result ?? "")
        }
    }

    private struct CommandGroup {
        let name: String
        let commands: [CommandInfo]
    }

    private var groups: [CommandGroup] {
        let query = search.trimmingCharacters(in: .whitespaces).lowercased()
        let shown = commands.filter { query.isEmpty || $0.label.lowercased().contains(query) || $0.id.lowercased().contains(query) }
        return Dictionary(grouping: shown) { $0.menu.first ?? "Other" }
            .map { CommandGroup(name: $0.key, commands: $0.value.sorted { $0.label < $1.label }) }
            .sorted { $0.name < $1.name }
    }

    private func load() async {
        guard let engine = model.engine else { return }
        if let all = try? await engine.commands() {
            commands = all
        }
    }

    private func run(_ command: CommandInfo, _ params: JSONValue) async {
        guard let answer = await model.run(command.id, params) else { return }
        if case .object(let o) = answer, o.isEmpty { return }
        result = prettyJSON(answer)
    }
}

private struct CommandRow: View {
    let command: CommandInfo

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack {
                Text(LocalizedStringKey(command.label))
                Spacer()
                if let shortcut = command.shortcut {
                    Text(shortcut).font(.caption).foregroundStyle(.secondary)
                }
            }
            if let why = command.disabledReason, !command.enabled {
                Text(why).font(.caption).foregroundStyle(.secondary)
            }
        }
    }
}

/// The parameters of a command that takes some, as JSON (the command documents them).
private struct CommandForm: View {
    let command: CommandInfo
    let run: (JSONValue) -> Void
    @State private var text = "{}"
    @State private var error: String?
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            Form {
                Section("Parameters") {
                    Text(command.params).font(.footnote.monospaced()).textSelection(.enabled)
                    TextEditor(text: $text)
                        .font(.body.monospaced())
                        .autocorrectionDisabled()
                        .noAutocap()
                        .frame(minHeight: 120)
                }
                if let error {
                    Text(error).foregroundStyle(.red)
                }
            }
            .navigationTitle(LocalizedStringKey(command.label))
            .inlineTitle()
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", systemImage: "xmark") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Run", systemImage: "checkmark") {
                        do {
                            run(try JSONDecoder().decode(JSONValue.self, from: Data(text.utf8)))
                        } catch {
                            self.error = String(localized: "That isn't JSON: \(error.localizedDescription)")
                        }
                    }
                }
            }
        }
    }
}

/// `value` as indented JSON, for showing.
func prettyJSON(_ value: JSONValue) -> String {
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
    return (try? encoder.encode(value)).flatMap { String(data: $0, encoding: .utf8) } ?? ""
}
