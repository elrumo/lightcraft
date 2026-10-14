import LightCraftCore
import SwiftUI

/// Settings ▸ Server Administration: the server's `/admin` console as screens. Signed out, the
/// admin's sign-in (or, on a server that has no admin yet, its setup); signed in, the Overview with
/// the Users, Search and Storage pages under it. The admin session is not the Sync sign-in: the server
/// keeps them apart, so the password is asked here (and kept by the host in memory only).
struct AdminView: View {
    @Environment(AppModel.self) private var model
    @State private var admin = AdminModel()
    @State private var restored = false

    var body: some View {
        Group {
            if !restored {
                ProgressView()
            } else {
                // (signed out, the page shows the sign-in form in its place: `adminPage`)
                AdminOverview(admin: admin)
            }
        }
        .navigationTitle("Server Administration")
        .inlineTitle()
        .task {
            admin.engine = model.engine
            await admin.restore()
            restored = true
        }
    }
}

/// The admin's sign-in, or the setup of a server that waits for its first administrator.
struct AdminSignIn: View {
    @Environment(AppModel.self) private var model
    let admin: AdminModel
    @State private var server = ""
    @State private var user = ""
    @State private var password = ""
    @State private var code = ""
    @State private var confirm = ""
    /// Does the server named wait for its first administrator? (`nil`: not known.)
    @State private var setup: Bool?
    @State private var working = false
    @FocusState private var focus: Field?

    private enum Field { case server, code, user, password, confirm }

    init(admin: AdminModel) {
        self.admin = admin
        // (a session that ended: the same server and user again)
        _server = State(initialValue: admin.server)
        _user = State(initialValue: admin.user)
    }

    var body: some View {
        Form {
            if let notice = admin.notice {
                Section { Text(notice).foregroundStyle(.orange) }
            }
            Section {
                TextField("Server", text: $server, prompt: Text(verbatim: "https://photos.example.com"))
                    .lcKeyboard(.url)
                    .textContentType(.URL)
                    .noAutocap()
                    .autocorrectionDisabled()
                    .focused($focus, equals: .server)
                    .submitLabel(.next)
                    .onSubmit { focus = setup == true ? .code : .user }
            } footer: {
                Text("The address of your LightCraft server, as in Settings ▸ Sync.")
            }
            if setup == true {
                Section {
                    TextField("Setup code", text: $code, prompt: Text(verbatim: "ABCD-1234"))
                        .lcAutocapCharacters()
                        .autocorrectionDisabled()
                        .focused($focus, equals: .code)
                    TextField("User name", text: $user)
                        .textContentType(.username)
                        .noAutocap()
                        .autocorrectionDisabled()
                        .focused($focus, equals: .user)
                    SecureField("Password", text: $password)
                        .textContentType(.newPassword)
                        .focused($focus, equals: .password)
                    SecureField("Confirm Password", text: $confirm)
                        .textContentType(.newPassword)
                        .focused($focus, equals: .confirm)
                } header: {
                    Text("Create the First Administrator")
                } footer: {
                    Text("This server has no administrator yet. Type the setup code it wrote in its log when it started. Administrators add the people who use the server and look after its devices and storage. Letters, digits, dots, dashes and underscores in the name; at least 8 characters in the password.")
                }
                Section {
                    Button("Create Administrator") { create() }
                        .disabled(working || !canCreate)
                }
            } else {
                Section {
                    TextField("User name", text: $user)
                        .textContentType(.username)
                        .noAutocap()
                        .autocorrectionDisabled()
                        .focused($focus, equals: .user)
                        .submitLabel(.next)
                        .onSubmit { focus = .password }
                    SecureField("Password", text: $password)
                        .textContentType(.password)
                        .focused($focus, equals: .password)
                        .submitLabel(.go)
                        .onSubmit { signIn() }
                } footer: {
                    Text("An administrator's user name and password. Sync's sign-in is not enough: the server asks again for this page.")
                }
                Section {
                    Button("Sign In") { signIn() }
                        .disabled(working || server.trimmingCharacters(in: .whitespaces).isEmpty || user.isEmpty || password.isEmpty)
                }
            }
        }
        .overlay { if working { ProgressView() } }
        .task {
            // the server and user Sync knows are the likeliest ones
            if server.isEmpty, let s = model.albums.sync, s.signedIn {
                server = s.server ?? ""
                user = s.user ?? ""
            }
            if server.isEmpty { server = admin.server }
            if user.isEmpty { user = admin.user }
            await check()
        }
        .onChange(of: focus) { old, _ in
            if old == .server { Task { await check() } }
        }
    }

    private var canCreate: Bool {
        !server.isEmpty && !code.trimmingCharacters(in: .whitespaces).isEmpty && AdminFormat.isUserName(user) && password.count >= 8 && password == confirm
    }

    private func check() async {
        let typed = server.trimmingCharacters(in: .whitespaces)
        guard !typed.isEmpty else { return }
        setup = await admin.needsSetup(server: typed)
    }

    private func signIn() {
        guard !working else { return }
        working = true
        Task {
            if await admin.signIn(server: server.trimmingCharacters(in: .whitespaces), user: user, password: password) {
                password = ""
            } else {
                // (a server that waits for its first admin answers the same as a wrong password)
                await check()
            }
            working = false
        }
    }

    private func create() {
        working = true
        Task {
            let made = await admin.setUp(server: server.trimmingCharacters(in: .whitespaces), code: code, user: user, password: password)
            if made { password = ""; confirm = "" }
            working = false
        }
    }
}

/// Overview: the server in numbers, how to connect a device to it, and the way to the other pages.
struct AdminOverview: View {
    let admin: AdminModel
    @State private var status: JSONValue = .null
    @State private var users: [JSONValue] = []

    var body: some View {
        let disk = status["disk"]
        List {
            Section {
                LabeledContent("Users", value: users.count.formatted())
                LabeledContent("Photos", value: users.reduce(0) { $0 + ($1["photos"].int ?? 0) }.formatted())
                LabeledContent("Devices Signed In", value: users.reduce(0) { $0 + ($1["devices"].int ?? 0) }.formatted())
                if let total = disk["total"].int, let free = disk["free"].int, total > 0 {
                    VStack(alignment: .leading, spacing: 6) {
                        LabeledContent("Free Disk Space", value: AdminFormat.bytes(.int(free)))
                        AdminMeter(fraction: 1 - Double(free) / Double(total))
                    }
                }
            }
            Section {
                NavigationLink {
                    AdminUsers(admin: admin)
                } label: {
                    Label("Users", systemImage: "person.2")
                }
                NavigationLink {
                    AdminSearch(admin: admin)
                } label: {
                    Label("Search & People", systemImage: "sparkle.magnifyingglass")
                }
                NavigationLink {
                    AdminStorage(admin: admin)
                } label: {
                    Label("Storage & Clean Up", systemImage: "internaldrive")
                }
            }
            Section {
                LabeledContent("Server Address") {
                    Text(admin.server).textSelection(.enabled).foregroundStyle(.secondary)
                }
                Button("Copy Address", systemImage: "doc.on.doc") { Platform.copy(admin.server) }
            } header: {
                Text("Connect a Device")
            } footer: {
                Text("Sync a library with this server from LightCraft on a Mac, PC or iPhone: open Settings ▸ Sync, enter the server address, and sign in with a user name and password from Users.")
            }
            Section {
                LabeledContent("Version", value: status["version"].string ?? "")
                LabeledContent("Data Folder") { Text(status["data"].string ?? "").font(.footnote.monospaced()).textSelection(.enabled) }
                LabeledContent("Listening On") { Text(status["listen"].string ?? "").font(.footnote.monospaced()).textSelection(.enabled) }
                LabeledContent("Web App") {
                    if let web = status["web"].string {
                        Text("Served from \(web)").foregroundStyle(.secondary)
                    } else {
                        Text("Not served").foregroundStyle(.secondary)
                    }
                }
            } header: {
                Text("Server")
            } footer: {
                Text("Set when the server starts: its data folder, address and domain come from the deployment (--data, --listen, LIGHTCRAFT_DOMAIN). Change them there and restart; moving a library under a running server can lose it.")
            }
        }
        .toolbar {
            ToolbarItem(placement: .lcTrailing) {
                Menu("Account", systemImage: "person.crop.circle") {
                    Text(admin.user)
                    Button("Sign Out", systemImage: "rectangle.portrait.and.arrow.right", role: .destructive) { Task { await admin.signOut() } }
                }
            }
        }
        .adminPage(admin)
        .adminTask(admin) { await load() }
        .refreshable { await load() }
    }

    /// The status and the users, and again while the server is still counting a user's storage.
    private func load() async {
        while !Task.isCancelled {
            await admin.attempt {
                status = try await admin.get("status")
                users = try await admin.get("users").array ?? []
            }
            guard users.contains(where: { $0["bytes"].isNull }) else { return }
            try? await Task.sleep(for: .milliseconds(1500))
        }
    }
}
