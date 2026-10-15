import LightCraftCore
import SwiftUI

/// Server Administration ▸ Limits: how much of the machine the server may use. Each box holds the
/// number an admin saved, or is empty for the automatic one (sized from the machine's cores and memory).
/// A box is locked when the deployment set it with a flag or environment variable. Nothing is dropped
/// over a limit: the work waits. The CPU threads count applies when the server restarts.
struct AdminLimits: View {
    let admin: AdminModel
    @State private var limits: JSONValue = .null
    /// What each box holds, by key; empty means automatic.
    @State private var fields: [String: String] = [:]
    @State private var saving = false
    @FocusState private var focus: String?

    private static let rows: [(key: String, label: String, help: String)] = [
        ("threads", String(localized: "CPU Threads"), String(localized: "Cores that decoding, developing and the search models share.")),
        ("previews", String(localized: "Previews at Once"), String(localized: "Photos whose previews are built side by side. The rest wait their turn.")),
        ("renders", String(localized: "Renders at Once"), String(localized: "Full-size exports for devices. Devices ask again when all are busy.")),
        ("requests", String(localized: "Requests at Once"), String(localized: "Requests answered side by side. Devices retry the others.")),
    ]

    var body: some View {
        List {
            if limits.isNull {
                Section { ProgressView().frame(maxWidth: .infinity) }
            } else {
                Section {
                    Text(machine).foregroundStyle(.secondary)
                }
                if restartPending {
                    Section {
                        Label("The server keeps using \(AdminFormat.number(limits["threadsNow"])) threads until it restarts, then \(AdminFormat.number(limits["threads"]["value"])).", systemImage: "arrow.clockwise")
                            .foregroundStyle(.orange)
                    }
                }
                ForEach(Self.rows, id: \.key) { row in
                    let r = limits[row.key]
                    let fixed = r["fixed"].string
                    Section {
                        LabeledContent(row.label) {
                            TextField(row.label, text: binding(row.key), prompt: Text("Automatic (\(AdminFormat.number(r["automatic"])))"))
                                .lcKeyboard(.number)
                                .multilineTextAlignment(.trailing)
                                .autocorrectionDisabled()
                                .disabled(fixed != nil)
                                .focused($focus, equals: row.key)
                        }
                    } footer: {
                        if let fixed {
                            Text("Set by \(fixed) when the server starts.")
                        } else {
                            Text(row.help)
                        }
                    }
                }
                Section {
                    Button("Save Limits") { save() }
                        .disabled(saving)
                } footer: {
                    Text("Leave a box empty for the automatic number. Nothing is dropped: work over a limit waits.")
                }
            }
        }
        .navigationTitle("Limits")
        .inlineTitle()
        .adminPage(admin)
        .adminTask(admin) { await load() }
        .refreshable { await load() }
    }

    private func binding(_ key: String) -> Binding<String> {
        Binding(get: { fields[key] ?? "" }, set: { fields[key] = $0 })
    }

    private var machine: String {
        let cores = limits["cores"].int ?? 0
        let memory = limits["memory"].int.map { _ in String(localized: ", \(AdminFormat.bytes(limits["memory"])) of memory") } ?? ""
        return String(localized: "How much of this machine the server may use (\(cores) cores\(memory)).")
    }

    /// CPU threads apply at the next restart: the number in effect now differs from the one that will be.
    private var restartPending: Bool {
        limits["threads"]["value"].int != limits["threadsNow"].int
    }

    /// The boxes show what the server says: the saved number, the locked one, or nothing for automatic.
    /// (Not while one is being typed in: a refresh must not take the text away.)
    private func load() async {
        await admin.attempt {
            let l = try await admin.get("limits")
            guard focus == nil else { return }
            show(l)
        }
    }

    private func show(_ l: JSONValue) {
        limits = l
        for row in Self.rows {
            let r = l[row.key]
            let number = r["fixed"].string != nil ? r["value"].int : r["saved"].int
            fields[row.key] = number.map { String($0) } ?? ""
        }
    }

    private func save() {
        // fixed boxes are left out of the body; an empty box is `null` (automatic)
        var body: [String: JSONValue] = [:]
        for row in Self.rows where limits[row.key]["fixed"].string == nil {
            let typed = (fields[row.key] ?? "").trimmingCharacters(in: .whitespaces)
            let max = limits[row.key]["max"].int ?? 1
            if typed.isEmpty {
                body[row.key] = .null
            } else if let n = Int64(typed), n >= 1, n <= max {
                body[row.key] = .int(n)
            } else {
                admin.failure = String(localized: "\(row.label): a number from 1 to \(max), or empty for automatic.")
                return
            }
        }
        focus = nil
        saving = true
        Task {
            await admin.attempt {
                show(try await admin.call("PUT", ["limits"], .object(body)))
                admin.info = String(localized: "Limits saved.")
            }
            saving = false
        }
    }
}
