#if os(iOS)
import LightCraftCore
import SwiftUI

/// New Smart Album and Edit Smart Album: its name and its rules (match all / any / none of a list
/// of field, operator, value, with nested groups). The fields, operators and starting values come
/// from the engine (`album.ruleFields`), the rules from `album.rules`, and what keeps them from
/// being saved from `album.checkRules`.
struct SmartAlbumSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    /// The album edited; `nil`: a new one (in `parent`).
    let album: UInt64?
    let parent: UInt64?
    @State private var name = ""
    @State private var rules: JSONValue = .null
    @State private var fields: [RuleField] = []
    @State private var problems: [String] = []

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Smart Album Name", text: $name)
                }
                if !rules.isNull {
                    RuleSetEditor(set: $rules, fields: fields, depth: 0)
                }
                if let problem = problems.first {
                    Section {
                        Label(problem, systemImage: "exclamationmark.triangle").foregroundStyle(.orange).font(.footnote)
                    }
                }
            }
            .navigationTitle(album == nil ? "New Smart Album" : "Edit Smart Album")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel", systemImage: "xmark") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button(album == nil ? "Create" : "Save") { save() }
                        .disabled(rules.isNull || !problems.isEmpty)
                }
            }
        }
        .task {
            guard let engine = model.engine else { return }
            fields = (try? await engine.execute("album.ruleFields", [:], as: [RuleField].self)) ?? []
            var p: [String: JSONValue] = [:]
            if let album { p["id"] = .int(Int64(clamping: album)) }
            if let r = try? await engine.execute("album.rules", .object(p)) {
                name = r["name"].string ?? ""
                rules = r["rules"]
            }
        }
        .task(id: rules) {
            guard let engine = model.engine, !rules.isNull else { return }
            var p: [String: JSONValue] = ["rules": rules]
            if let album { p["id"] = .int(Int64(clamping: album)) }
            problems = ((try? await engine.execute("album.checkRules", .object(p)))?.array ?? []).compactMap { $0["message"].string }
        }
    }

    private func save() {
        let title = name.trimmingCharacters(in: .whitespaces).isEmpty ? String(localized: "Smart Album") : name.trimmingCharacters(in: .whitespaces)
        Task {
            let done: JSONValue?
            if let album {
                // the name and the rules together: one undo step
                done = await model.run("album.setRules", ["id": .int(Int64(clamping: album)), "name": .string(title), "replace": true, "rules": ["ruleSet": rules]])
            } else {
                var p: [String: JSONValue] = ["name": .string(title), "rules": ["ruleSet": rules]]
                if let parent { p["parent"] = .int(Int64(clamping: parent)) }
                done = await model.run("album.createSmart", .object(p))
            }
            if done != nil { dismiss() }
        }
    }
}

/// A field a rule can test (`album.ruleFields`).
nonisolated struct RuleField: Decodable, Hashable {
    var field: String
    var label: String
    /// `text`, `keywords`, `number`, `date`, `choice`, `bool` or `album`.
    var kind: String
    /// Its submenu in the field menu (`nil`: the top level).
    var group: String?
    var ops: [RuleOp]
    var choices: [String]?
    var choiceLabels: [String: String]?

    nonisolated struct RuleOp: Decodable, Hashable {
        var op: String
        var label: String
        var `default`: JSONValue
    }
}

/// A rule set: how its rules combine, the rules (a group opens a page of its own), Add Rule, Add Group.
private struct RuleSetEditor: View {
    @Binding var set: JSONValue
    let fields: [RuleField]
    let depth: Int

    var body: some View {
        let rules = set["rules"].array ?? []
        Section {
            Picker("Match", selection: Binding(get: { set["match"].string ?? "all" }, set: { set["match"] = .string($0) })) {
                Text("All of these").tag("all")
                Text("Any of these").tag("any")
                Text("None of these").tag("none")
            }
            ForEach(rules.indices, id: \.self) { i in
                if rules[i]["group"].isNull {
                    RuleRow(rule: rule(i), fields: fields)
                } else {
                    NavigationLink {
                        // (a group's editor is this one, a level down)
                        AnyView(Form { RuleSetEditor(set: group(i), fields: fields, depth: depth + 1) }.navigationTitle("Group"))
                    } label: {
                        Label("Group of \(rules[i]["group"]["rules"].array?.count ?? 0) rules", systemImage: "square.stack.3d.up")
                    }
                }
            }
            .onDelete { offsets in
                var all = rules
                all.remove(atOffsets: offsets)
                set["rules"] = .array(all)
            }
            Menu {
                Button("Rule", systemImage: "plus") { append(Self.newRule) }
                if depth < 3 {
                    Button("Group", systemImage: "square.stack.3d.up") { append(["group": ["match": "any", "rules": [Self.newRule]]]) }
                }
            } label: {
                Label("Add", systemImage: "plus.circle")
            }
        } header: {
            Text(depth == 0 ? "Rules" : "Rules in this group")
        }
    }

    /// Rating ≥ 3, as a new rule starts.
    static let newRule: JSONValue = ["field": "rating", "op": "gte", "value": 3]

    private func append(_ rule: JSONValue) {
        set["rules"] = .array((set["rules"].array ?? []) + [rule])
    }

    private func rule(_ i: Int) -> Binding<JSONValue> {
        Binding(get: { set["rules"].array?[safe: i] ?? .null }, set: { new in
            var all = set["rules"].array ?? []
            guard all.indices.contains(i) else { return }
            all[i] = new
            set["rules"] = .array(all)
        })
    }

    private func group(_ i: Int) -> Binding<JSONValue> {
        Binding(get: { set["rules"].array?[safe: i]?["group"] ?? .null }, set: { new in
            var all = set["rules"].array ?? []
            guard all.indices.contains(i) else { return }
            all[i]["group"] = new
            set["rules"] = .array(all)
        })
    }
}

/// One rule: its field (a menu, grouped as the engine groups them), its operator and its value.
private struct RuleRow: View {
    @Environment(AppModel.self) private var model
    @Binding var rule: JSONValue
    let fields: [RuleField]

    var body: some View {
        let field = fields.first { $0.field == rule["field"].string }
        let op = rule["op"].string ?? ""
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Menu {
                    ForEach(fields.filter { $0.group == nil }, id: \.field) { f in
                        Button(f.label) { choose(f) }
                    }
                    ForEach(groups, id: \.self) { g in
                        Menu(g) {
                            ForEach(fields.filter { $0.group == g }, id: \.field) { f in
                                Button(f.label) { choose(f) }
                            }
                        }
                    }
                } label: {
                    Text(field?.label ?? rule["field"].string ?? "").fontWeight(.medium)
                }
                Spacer()
                if let field {
                    Picker("Operator", selection: Binding(get: { op }, set: { setOp($0, field) })) {
                        ForEach(field.ops, id: \.op) { Text($0.label).tag($0.op) }
                    }
                    .labelsHidden()
                }
            }
            if let field, op != "isEmpty", op != "isNotEmpty" {
                value(field, op)
            }
        }
    }

    private var groups: [String] {
        var out: [String] = []
        for f in fields {
            if let g = f.group, !out.contains(g) { out.append(g) }
        }
        return out
    }

    private func choose(_ f: RuleField) {
        guard let first = f.ops.first else { return }
        rule = ["field": .string(f.field), "op": .string(first.op), "value": first.default]
    }

    /// A new operator keeps the value when it has the same shape, else starts from its default.
    private func setOp(_ op: String, _ f: RuleField) {
        let start = f.ops.first { $0.op == op }?.default ?? .null
        var r = rule
        r["op"] = .string(op)
        if Self.shape(start) != Self.shape(rule["value"]) { r["value"] = start }
        rule = r
    }

    private static func shape(_ v: JSONValue) -> Int {
        if v.isNull { return 0 }
        if v.array != nil { return 1 }
        if v["n"].int != nil { return 2 }
        return 3
    }

    @ViewBuilder private func value(_ f: RuleField, _ op: String) -> some View {
        let value = Binding(get: { rule["value"] }, set: { rule["value"] = $0 })
        switch f.kind {
        case "bool":
            Toggle("Yes", isOn: Binding(get: { value.wrappedValue.bool ?? true }, set: { value.wrappedValue = .bool($0) }))
        case "choice":
            Picker("Value", selection: Binding(get: { value.wrappedValue.string ?? "" }, set: { value.wrappedValue = .string($0) })) {
                ForEach(f.choices ?? [], id: \.self) { Text(f.choiceLabels?[$0] ?? $0).tag($0) }
            }
        case "album":
            Picker("Album", selection: Binding(get: { value.wrappedValue.int ?? 0 }, set: { value.wrappedValue = .int($0) })) {
                Text("Choose…").tag(Int64(0))
                ForEach(model.albums.albumsAndSmart) { a in Text(a.name).tag(Int64(clamping: a.id)) }
            }
        case "date" where op == "inLast" || op == "notInLast":
            HStack {
                Stepper(value: Binding(get: { Int(value.wrappedValue["n"].int ?? 30) }, set: { value.wrappedValue["n"] = .int(Int64($0)) }), in: 1...9999) {
                    Text(verbatim: "\(value.wrappedValue["n"].int ?? 30)")
                }
                Picker("Unit", selection: Binding(get: { value.wrappedValue["unit"].string ?? "days" }, set: { value.wrappedValue["unit"] = .string($0) })) {
                    Text("Days").tag("days")
                    Text("Weeks").tag("weeks")
                    Text("Months").tag("months")
                    Text("Years").tag("years")
                }
                .labelsHidden()
            }
        default:
            if op == "between" {
                HStack {
                    field(Binding(get: { value.wrappedValue.array?[safe: 0] ?? "" }, set: { pair(value, 0, $0) }), f)
                    Text(verbatim: "–")
                    field(Binding(get: { value.wrappedValue.array?[safe: 1] ?? "" }, set: { pair(value, 1, $0) }), f)
                }
            } else {
                field(value, f)
            }
        }
    }

    private func pair(_ value: Binding<JSONValue>, _ i: Int, _ v: JSONValue) {
        var a = value.wrappedValue.array ?? [.null, .null]
        while a.count < 2 { a.append(.null) }
        a[i] = v
        value.wrappedValue = .array(a)
    }

    /// A text field for a text, number or date value (numbers are kept as numbers, a shutter speed
    /// as written).
    private func field(_ v: Binding<JSONValue>, _ f: RuleField) -> some View {
        let text = Binding<String>(get: {
            if let s = v.wrappedValue.string { return s }
            if let n = v.wrappedValue.double { return n.rounded() == n ? String(Int64(n)) : String(n) }
            return ""
        }, set: { s in
            if f.kind == "number", !s.contains("/"), let n = Double(s) {
                v.wrappedValue = n.rounded() == n ? .int(Int64(n)) : .double(n)
            } else {
                v.wrappedValue = .string(s)
            }
        })
        return TextField(f.kind == "date" ? "YYYY-MM-DD" : String(localized: "Value"), text: text)
            .keyboardType(f.kind == "number" ? .numbersAndPunctuation : .default)
            .textInputAutocapitalization(.never)
    }
}

extension Array {
    subscript(safe i: Int) -> Element? {
        indices.contains(i) ? self[i] : nil
    }
}
#endif
