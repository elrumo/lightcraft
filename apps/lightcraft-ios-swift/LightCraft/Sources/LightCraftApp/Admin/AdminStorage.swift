#if os(iOS)
import LightCraftCore
import SwiftUI

/// Server Administration ▸ Storage & Clean Up: the server's disk, what each user's photos take on
/// it, and the cleaning of photo files no library uses any more.
struct AdminStorage: View {
    let admin: AdminModel
    @State private var status: JSONValue = .null
    @State private var users: [JSONValue] = []
    @State private var tick = 0
    @State private var checking = false
    @State private var found: Cleaning?

    /// What the last check or clean-up found.
    private struct Cleaning {
        enum Kind { case info, good, bad }
        var kind: Kind
        var title: String
        var text: String
        /// Unused files a check found (the button to remove them shows).
        var removable = 0
    }

    var body: some View {
        let disk = status["disk"]
        let sorted = users.sorted { ($0["bytes"].int ?? 0) > ($1["bytes"].int ?? 0) }
        let top = Double(sorted.first?["bytes"].int ?? 0)
        List {
            Section {
                if let total = disk["total"].int, let free = disk["free"].int, total > 0 {
                    let used = max(total - free, 0)
                    VStack(alignment: .leading, spacing: 6) {
                        HStack {
                            Text("\(AdminFormat.bytes(.int(used))) used")
                            Spacer()
                            Text("\(Int((100 * Double(used) / Double(total)).rounded()))%").foregroundStyle(.secondary)
                        }
                        AdminMeter(fraction: Double(used) / Double(total))
                        Text("\(AdminFormat.bytes(.int(free))) free of \(AdminFormat.bytes(.int(total))), on the disk that holds \(status["data"].string ?? "").")
                            .font(.footnote).foregroundStyle(.secondary)
                    }
                } else if !status.isNull {
                    Text("Disk usage isn't available on this system.").foregroundStyle(.secondary)
                }
            } header: {
                Text("Disk")
            } footer: {
                Text("Disk space on the server, and what each user's photos take on it: the files kept for them, and the photos in their library folders.")
            }
            Section("By User") {
                if sorted.isEmpty { Text("No users yet.").foregroundStyle(.secondary) }
                ForEach(sorted, id: \.self) { u in
                    VStack(alignment: .leading, spacing: 6) {
                        HStack {
                            AdminAvatar(name: u["name"].string ?? "", size: 26)
                            Text(u["name"].string ?? "")
                            Spacer()
                            Text(AdminFormat.bytes(u["bytes"])).foregroundStyle(.secondary)
                        }
                        AdminMeter(fraction: top > 0 ? Double(u["bytes"].int ?? 0) / top : 0, warns: false)
                        if let parts = AdminFormat.storageParts(u) {
                            Text(parts).font(.caption).foregroundStyle(.secondary)
                        }
                    }
                }
            }
            Section {
                Button("Check for Unused Files", systemImage: "sparkles") { check() }
                    .disabled(checking)
                if let found {
                    VStack(alignment: .leading, spacing: 4) {
                        Text(found.title).font(.headline)
                        Text(found.text).font(.subheadline)
                    }
                    .foregroundStyle(found.kind == .bad ? Color.red : found.kind == .good ? .green : .primary)
                    if found.removable > 0 {
                        Button("Remove Files…", systemImage: "trash", role: .destructive) { admin.offer = removeOffer(found.removable) }
                    }
                }
            } header: {
                Text("Clean Up")
            } footer: {
                Text("Finds photo files that no library uses any more (photos deleted for good) and uploads that never finished, if they are older than a day. Checking changes nothing.")
            }
        }
        .navigationTitle("Storage")
        .navigationBarTitleDisplayMode(.inline)
        .adminPage(admin)
        .adminTask(admin, tick) { await load() }
        .refreshable { await load() }
    }

    /// The disk and the users, and again while the server is still counting someone's storage.
    private func load() async {
        while !Task.isCancelled {
            guard await admin.attempt({
                status = try await admin.get("status")
                users = try await admin.get("users").array ?? []
            }) else { return }
            guard users.contains(where: { $0["bytes"].isNull }) else { return }
            try? await Task.sleep(for: .milliseconds(1500))
        }
    }

    private static func problems(_ r: JSONValue) -> String {
        let errors = (r["errors"].array ?? []).compactMap(\.string)
        guard !errors.isEmpty else { return "" }
        let list = errors.joined(separator: "; ")
        return errors.count == 1 ? String(localized: " 1 problem: \(list)") : String(localized: " \(errors.count) problems: \(list)")
    }

    private static func files(_ n: Int64) -> String {
        n == 1 ? String(localized: "1 file") : String(localized: "\(n) files")
    }

    /// A dry run: what would be removed. Nothing is removed (the body is spelt out: the server treats
    /// anything it can't read as a real clean-up).
    private func check() {
        checking = true
        Task {
            await admin.attempt {
                let r = try await admin.call("POST", ["gc"], ["dryRun": true])
                let n = r["removed"].int ?? 0, problems = Self.problems(r)
                if n > 0 {
                    let unused = n == 1 ? String(localized: "1 unused file") : String(localized: "\(n) unused files")
                    found = Cleaning(
                        kind: .info,
                        title: String(localized: "\(unused), \(AdminFormat.bytes(r["bytes"]))"),
                        text: String(localized: "Removing them frees this space. It cannot be undone.") + problems,
                        removable: Int(n)
                    )
                } else {
                    found = Cleaning(
                        kind: problems.isEmpty ? .good : .bad,
                        title: String(localized: "Nothing to clean up"),
                        text: String(localized: "Every photo file on the server is in use.") + problems
                    )
                }
            }
            checking = false
        }
    }

    private func removeOffer(_ n: Int) -> AdminOffer {
        AdminOffer(
            title: String(localized: "Remove \(n == 1 ? String(localized: "1 unused file") : String(localized: "\(n) unused files"))?"),
            text: String(localized: "No library uses them any more. This can't be undone."),
            ok: String(localized: "Remove Files"),
            danger: true
        ) {
            await admin.attempt {
                let r = try await admin.call("POST", ["gc"], ["dryRun": false])
                let problems = Self.problems(r)
                found = Cleaning(
                    kind: problems.isEmpty ? .good : .bad,
                    title: String(localized: "Removed \(Self.files(r["removed"].int ?? 0))"),
                    text: String(localized: "Freed \(AdminFormat.bytes(r["bytes"])).") + problems
                )
            }
            tick += 1
        }
    }
}
#endif
