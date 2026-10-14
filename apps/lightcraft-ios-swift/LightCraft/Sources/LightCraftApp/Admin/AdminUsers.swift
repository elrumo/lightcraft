#if os(iOS)
import LightCraftCore
import SwiftUI

/// Server Administration ▸ Users: who uses the server, and Add User.
struct AdminUsers: View {
    let admin: AdminModel
    @State private var users: [JSONValue] = []
    @State private var loaded = false
    @State private var adding = false
    @State private var tick = 0

    var body: some View {
        List {
            if loaded && users.isEmpty {
                Text("No users yet. Add the people who use this server.").foregroundStyle(.secondary)
            }
            ForEach(users, id: \.self) { u in
                let name = u["name"].string ?? ""
                NavigationLink {
                    AdminUserDetail(name: name, admin: admin)
                } label: {
                    AdminUserRow(user: u, you: name == admin.user)
                }
            }
        }
        .navigationTitle("Users")
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button("Add User", systemImage: "plus") { adding = true }
            }
        }
        .sheet(isPresented: $adding, onDismiss: { tick += 1 }) { AdminAddUser(admin: admin) }
        .adminPage(admin)
        .adminTask(admin, tick) { await load() }
        .refreshable { await load() }
    }

    /// The users, and again while the server is still counting someone's storage.
    private func load() async {
        while !Task.isCancelled {
            await admin.attempt { users = try await admin.get("users").array ?? [] }
            loaded = true
            guard users.contains(where: { $0["bytes"].isNull }) else { return }
            try? await Task.sleep(for: .milliseconds(1500))
        }
    }
}

struct AdminUserRow: View {
    let user: JSONValue
    let you: Bool

    var body: some View {
        let name = user["name"].string ?? ""
        let sub = [user["admin"].bool == true ? String(localized: "Administrator") : nil, you ? String(localized: "You") : nil]
            .compactMap { $0 }.joined(separator: " · ")
        HStack(spacing: 12) {
            AdminAvatar(name: name)
            VStack(alignment: .leading, spacing: 2) {
                Text(name)
                if !sub.isEmpty { Text(sub).font(.caption).foregroundStyle(.secondary) }
            }
            Spacer()
            VStack(alignment: .trailing, spacing: 2) {
                Text("^[\(Int(user["photos"].int ?? 0)) photo](inflect: true)")
                Text(AdminFormat.bytes(user["bytes"])).font(.caption).foregroundStyle(.secondary)
            }
            .font(.subheadline)
        }
    }
}

/// Add User: a name, a password (at least 8 characters) and whether they administer too.
struct AdminAddUser: View {
    let admin: AdminModel
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var password = ""
    @State private var makeAdmin = false
    @State private var working = false

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("User name", text: $name)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    SecureField("Password", text: $password)
                        .textContentType(.newPassword)
                    Toggle("Administrator", isOn: $makeAdmin)
                } footer: {
                    Text("They sign in to LightCraft on their devices with this user name and password, and get a library of their own. Letters, digits, dots, dashes and underscores in the name; at least 8 characters in the password, which you give them privately. Administrators can sign in to this page to manage users, devices and storage.")
                }
            }
            .navigationTitle("Add User")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel", systemImage: "xmark") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Add") { add() }.disabled(working || !AdminFormat.isUserName(name) || password.count < 8)
                }
            }
            .adminPage(admin)
        }
    }

    private func add() {
        working = true
        Task {
            let body: JSONValue = ["name": .string(name), "password": .string(password), "admin": .bool(makeAdmin)]
            if await admin.attempt({ try await admin.call("POST", ["users"], body) }) { dismiss() }
            working = false
        }
    }
}

/// One user: their numbers, the pages for their devices, library folders and shared links, and what
/// can be done to them.
struct AdminUserDetail: View {
    let name: String
    let admin: AdminModel
    @Environment(\.dismiss) private var dismiss
    @State private var users: [JSONValue] = []
    @State private var loaded = false
    @State private var tick = 0
    @State private var askPassword = false
    @State private var password = ""
    @State private var askLimit = false
    @State private var limit = ""
    @State private var askRemove = false
    @State private var typed = ""

    private var u: JSONValue { users.first { $0["name"].string == name } ?? .null }
    private var onlyAdmin: Bool { u["admin"].bool == true && users.filter { $0["admin"].bool == true }.count < 2 }

    var body: some View {
        let isAdmin = u["admin"].bool == true
        List {
            if u.isNull {
                if loaded { Text("There is no user \(name).") } else { ProgressView() }
            } else {
                Section {
                    HStack(spacing: 12) {
                        AdminAvatar(name: name, size: 48)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(name).font(.title3.weight(.semibold))
                            Text(name == admin.user ? (isAdmin ? "Administrator · You" : "You") : (isAdmin ? "Administrator" : "User"))
                                .font(.subheadline).foregroundStyle(.secondary)
                        }
                    }
                }
                Section {
                    LabeledContent("Photos", value: AdminFormat.number(u["photos"]))
                    LabeledContent("Albums", value: AdminFormat.number(u["albums"]))
                    storage
                }
                Section {
                    NavigationLink {
                        AdminDevices(name: name, admin: admin)
                    } label: {
                        LabeledContent("Devices", value: AdminFormat.number(u["devices"]))
                    }
                    NavigationLink {
                        AdminFolders(name: name, admin: admin)
                    } label: {
                        LabeledContent("Library Folders", value: (u["folders"].array?.count ?? 0).formatted())
                    }
                    NavigationLink {
                        AdminShares(name: name, admin: admin)
                    } label: {
                        LabeledContent("Shared Links", value: AdminFormat.number(u["shares"]))
                    }
                }
                Section {
                    Button("Reset Password…", systemImage: "key") {
                        password = ""
                        askPassword = true
                    }
                    .alert("Reset password", isPresented: $askPassword) {
                        SecureField("New password", text: $password)
                        Button("Set Password") { setPassword() }.disabled(password.count < 8)
                        Button("Cancel", role: .cancel) {}
                    } message: {
                        Text("Choose a new password for \(name), at least 8 characters. Their devices stay signed in; the new password is for signing in again.")
                    }
                    Button("Storage Limit…", systemImage: "internaldrive") {
                        let quota = u["quota"].double ?? 0
                        limit = quota > 0 ? String((quota / 1e8).rounded() / 10) : ""
                        askLimit = true
                    }
                    .alert("Storage limit for \(name)", isPresented: $askLimit) {
                        TextField("No limit", text: $limit).keyboardType(.decimalPad)
                        Button("Set Limit") { setLimit() }.disabled(parsedLimit == nil)
                        Button("Cancel", role: .cancel) {}
                    } message: {
                        Text("The most the server keeps for \(name), in GB (at least 0.1): the originals their devices upload, previews, and photos added through their shared links. Photos in library folders don't count. Over the limit, uploads wait until there is room; nothing is deleted. Leave it empty for no limit.")
                    }
                    Button(isAdmin ? "Remove Administrator Access…" : "Make Administrator…", systemImage: isAdmin ? "person.badge.minus" : "person.badge.shield.checkmark") {
                        admin.offer = setAdminOffer(!isAdmin)
                    }
                    .disabled(onlyAdmin)
                    Button("Remove User…", systemImage: "trash", role: .destructive) {
                        typed = ""
                        askRemove = true
                    }
                    .disabled(onlyAdmin)
                    .alert("Remove \(name)?", isPresented: $askRemove) {
                        TextField("Type \(name) to confirm", text: $typed).textInputAutocapitalization(.never).autocorrectionDisabled()
                        Button("Remove User", role: .destructive) { remove() }.disabled(typed != name)
                        Button("Cancel", role: .cancel) {}
                    } message: {
                        Text("Their devices are signed out and they can no longer sign in. Their library and photo files stay on the server, in its data folder, until you delete them by hand.")
                    }
                } footer: {
                    if onlyAdmin { Text("\(name) is the only administrator.") }
                }
            }
        }
        .navigationTitle(name)
        .navigationBarTitleDisplayMode(.inline)
        .adminPage(admin)
        .adminTask(admin, tick) { await load() }
        .refreshable { await load() }
        .onChange(of: loaded && u.isNull && admin.signedIn) { _, gone in if gone { dismiss() } }
    }

    /// Storage used, what it is made of, and how much of the limit.
    private var storage: some View {
        VStack(alignment: .leading, spacing: 6) {
            LabeledContent("Storage Used", value: AdminFormat.bytes(u["bytes"]))
            if let parts = AdminFormat.storageParts(u) {
                Text(parts).font(.caption).foregroundStyle(.secondary)
            }
            // (the limit counts what LightCraft keeps for them, not their library folders)
            if let quota = u["quota"].int, quota > 0 {
                let kept = max((u["bytes"].int ?? 0) - (u["folderBytes"].int ?? 0), 0)
                Text("\(u["bytes"].isNull ? "…" : AdminFormat.bytes(.int(kept))) of \(AdminFormat.bytes(.int(quota))) allowed")
                    .font(.caption).foregroundStyle(.secondary)
                AdminMeter(fraction: Double(kept) / Double(quota))
            }
        }
    }

    private func load() async {
        while !Task.isCancelled {
            await admin.attempt { users = try await admin.get("users").array ?? [] }
            loaded = true
            guard users.contains(where: { $0["bytes"].isNull }) else { return }
            try? await Task.sleep(for: .milliseconds(1500))
        }
    }

    private func setPassword() {
        let p = password
        Task {
            if await admin.attempt({ try await admin.call("PUT", ["users", name, "password"], ["password": .string(p)]) }) {
                admin.info = String(localized: "\(name)'s password was changed.")
            }
        }
    }

    /// The limit typed, in GB: 0 for none, `nil` when it isn't a size of at least 0.1.
    private var parsedLimit: Double? {
        let t = limit.trimmingCharacters(in: .whitespaces).replacingOccurrences(of: ",", with: ".")
        if t.isEmpty { return 0 }
        guard let gb = Double(t), gb >= 0.1, gb < 1e6 else { return nil }
        return gb
    }

    private func setLimit() {
        guard let gb = parsedLimit else { return }
        Task {
            let quota: JSONValue = gb == 0 ? .null : .int(Int64((gb * 1e9).rounded()))
            await admin.attempt { try await admin.call("PUT", ["users", name, "quota"], ["quota": quota]) }
            tick += 1
        }
    }

    private func setAdminOffer(_ on: Bool) -> AdminOffer {
        AdminOffer(
            title: on ? String(localized: "Make \(name) an administrator?") : String(localized: "Remove administrator access?"),
            text: on
                ? String(localized: "Administrators can sign in here to add and remove users, sign out devices and clean up storage.")
                : String(localized: "\(name) keeps their library and devices, but can no longer sign in to this page."),
            ok: on ? String(localized: "Make Administrator") : String(localized: "Remove Access"),
            danger: !on
        ) {
            await admin.attempt { try await admin.call("PUT", ["users", name, "admin"], ["admin": .bool(on)]) }
            tick += 1
        }
    }

    private func remove() {
        Task {
            var files = ""
            let removed = await admin.attempt { files = try await admin.call("DELETE", ["users", name])["files"].string ?? "" }
            if removed {
                admin.info = String(localized: "Removed \(name). Their files stay in \(files).")
                dismiss()
            }
        }
    }
}

/// The devices signed in as a user, each with Sign Out.
struct AdminDevices: View {
    let name: String
    let admin: AdminModel
    @State private var devices: [JSONValue] = []
    @State private var loaded = false
    @State private var tick = 0

    var body: some View {
        let now = Date.now.timeIntervalSince1970
        List {
            if loaded && devices.isEmpty { Text("No devices are signed in as \(name).").foregroundStyle(.secondary) }
            ForEach(devices, id: \.self) { d in
                let id = d["id"].int ?? 0
                let label = d["name"].string.flatMap { $0.isEmpty ? nil : $0 } ?? String(localized: "Device \(id)")
                VStack(alignment: .leading, spacing: 3) {
                    Label(label, systemImage: "iphone").font(.headline)
                    Text("Device \(id) · ID space \(d["space"].int ?? 0)").font(.caption).foregroundStyle(.secondary)
                    if let created = d["created"].int, created > 0 {
                        Text("Signed in \(AdminFormat.ago(created))").font(.caption).foregroundStyle(.secondary)
                    }
                    if let seen = d["lastSeen"].double {
                        if now - seen < 300 {
                            Text("Active now").font(.caption).foregroundStyle(.green)
                        } else {
                            Text("Last seen \(AdminFormat.ago(Int64(seen)))").font(.caption).foregroundStyle(.secondary)
                        }
                    } else {
                        Text("Not since the server started").font(.caption).foregroundStyle(.secondary)
                    }
                }
                .swipeActions {
                    Button("Sign Out") { admin.offer = signOut(d, label) }.tint(.red)
                }
                .contextMenu {
                    Button("Sign Out", systemImage: "rectangle.portrait.and.arrow.right", role: .destructive) { admin.offer = signOut(d, label) }
                }
            }
        }
        .navigationTitle("Devices")
        .navigationBarTitleDisplayMode(.inline)
        .adminPage(admin)
        .adminTask(admin, tick) { await load() }
        .refreshable { await load() }
    }

    private func load() async {
        await admin.attempt { devices = try await admin.get("users", name, "devices").array ?? [] }
        loaded = true
    }

    private func signOut(_ d: JSONValue, _ label: String) -> AdminOffer {
        AdminOffer(
            title: String(localized: "Sign out \(label)?"),
            text: String(localized: "It stops syncing right away. It keeps its library and can sign in again with \(name)'s password."),
            ok: String(localized: "Sign Out"),
            danger: true
        ) {
            await admin.attempt { try await admin.call("DELETE", ["users", name, "devices", String(d["id"].int ?? 0)]) }
            tick += 1
        }
    }
}

/// The links a user shared, each with Revoke (the link's secret is never shown).
struct AdminShares: View {
    let name: String
    let admin: AdminModel
    @State private var shares: [JSONValue] = []
    @State private var loaded = false
    @State private var tick = 0

    var body: some View {
        let now = Date.now.timeIntervalSince1970
        List {
            if loaded && shares.isEmpty { Text("\(name) has no shared links.").foregroundStyle(.secondary) }
            ForEach(shares, id: \.self) { s in
                let flags = [
                    s["originals"].bool == true ? String(localized: "Originals can be downloaded") : String(localized: "Pictures only"),
                    s["password"].bool == true ? String(localized: "Password") : nil,
                    s["upload"].bool == true ? String(localized: "Visitors add photos") : nil,
                ].compactMap { $0 }.joined(separator: " · ")
                VStack(alignment: .leading, spacing: 3) {
                    Label(s["name"].string ?? "", systemImage: "link").font(.headline)
                    Text(flags).font(.caption).foregroundStyle(.secondary)
                    if let made = s["created"].int { Text("Made \(AdminFormat.ago(made))").font(.caption).foregroundStyle(.secondary) }
                    if let ends = s["expires"].double, ends > 0 {
                        Text(ends < now ? String(localized: "Ended") : String(localized: "Ends in \(AdminFormat.duration(Int64(ends - now)))"))
                            .font(.caption).foregroundStyle(.secondary)
                    } else {
                        Text("Never ends").font(.caption).foregroundStyle(.secondary)
                    }
                }
                .swipeActions {
                    Button("Revoke") { admin.offer = revoke(s) }.tint(.red)
                }
                .contextMenu {
                    Button("Revoke", systemImage: "trash", role: .destructive) { admin.offer = revoke(s) }
                }
            }
        }
        .navigationTitle("Shared Links")
        .navigationBarTitleDisplayMode(.inline)
        .adminPage(admin)
        .adminTask(admin, tick) { await load() }
        .refreshable { await load() }
    }

    private func load() async {
        await admin.attempt { shares = try await admin.get("users", name, "shares").array ?? [] }
        loaded = true
    }

    private func revoke(_ s: JSONValue) -> AdminOffer {
        let album = s["name"].string ?? ""
        let id = s["id"].string ?? String(s["id"].int ?? 0)
        return AdminOffer(
            title: String(localized: "Revoke the link to \(album)?"),
            text: String(localized: "Anyone who has it stops seeing the album at once. \(name) can make a new link."),
            ok: String(localized: "Revoke"),
            danger: true
        ) {
            await admin.attempt { try await admin.call("DELETE", ["users", name, "shares", id]) }
            tick += 1
        }
    }
}
#endif
