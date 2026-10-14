#if os(iOS)
import LightCraftCore
import SwiftUI

/// Server administration (Settings ▸ Server Administration): the web console at the server's
/// `/admin`, as screens. The HTTP is the host's (`host.admin.*`, `crates/apphost/src/host/admin.rs`:
/// the admin session lives there, in memory, and its requests run off the engine thread); this holds
/// what the screens share: whether the admin is signed in, a failure to show, and the calls.
@MainActor @Observable
final class AdminModel {
    var engine: Engine?
    private(set) var signedIn = false
    private(set) var server = ""
    private(set) var user = ""
    /// Why the admin is back at the sign-in form ("Your session ended").
    var notice: String?
    /// An error to show (an alert on the page that is up).
    var failure: String?
    /// Something done that shows nowhere else (an alert: Settings is a sheet, under the app's toasts).
    var info: String?
    /// A question the page asks before it does something (an alert with the action's button).
    var offer: AdminOffer?

    /// The session the host still holds (it lives as long as the app does).
    func restore() async {
        guard let r = try? await ask("host.admin.session") else { return }
        signedIn = r["signedIn"].bool == true
        server = r["server"].string ?? ""
        user = r["user"].string ?? ""
    }

    func signIn(server: String, user: String, password: String) async -> Bool {
        let signed = await attempt {
            let r = try await ask("host.admin.signIn", ["server": .string(server), "user": .string(user), "password": .string(password)])
            self.server = r["server"].string ?? server
            self.user = r["user"].string ?? user
            notice = nil
            signedIn = true
        }
        return signed
    }

    func signOut() async {
        _ = try? await ask("host.admin.signOut")
        signedIn = false
    }

    /// `GET /api/admin/state` of a server nobody has signed in to: does it wait for its first admin?
    func needsSetup(server: String) async -> Bool? {
        let r = try? await ask("host.admin.call", ["method": "GET", "path": ["state"], "server": .string(server)])
        return r?["setup"].bool
    }

    /// The first administrator of a new server (the setup code is in the server's log), then signed in.
    func setUp(server: String, code: String, user: String, password: String) async -> Bool {
        let made = await attempt {
            let body: JSONValue = ["code": .string(code), "user": .string(user), "password": .string(password)]
            try await ask("host.admin.call", ["method": "POST", "path": ["setup"], "body": body, "server": .string(server)])
        }
        guard made else { return false }
        return await signIn(server: server, user: user, password: password)
    }

    /// An admin route (`path`: the segments after `/api/admin/`). An ended session sends the admin back to
    /// the sign-in form.
    @discardableResult
    func call(_ verb: String, _ path: [String], _ body: JSONValue = .null) async throws -> JSONValue {
        var params: JSONValue = ["method": .string(verb), "path": .array(path.map { .string($0) })]
        if !body.isNull { params["body"] = body }
        do {
            return try await ask("host.admin.call", params)
        } catch let e as ErrorBody where e.code == "unauthorized" {
            signedIn = false
            notice = String(localized: "Your session ended. Sign in again.")
            throw e
        }
    }

    func get(_ path: String...) async throws -> JSONValue {
        try await call("GET", path)
    }

    /// Run `work`: a failure is shown on the page (an ended session already sent the admin back to
    /// the sign-in form). Whether it worked.
    @discardableResult
    func attempt(_ work: () async throws -> Void) async -> Bool {
        do {
            try await work()
            return true
        } catch {
            if (error as? ErrorBody)?.code != "unauthorized" { failure = error.localizedDescription }
            return false
        }
    }

    @discardableResult
    private func ask(_ method: String, _ params: JSONValue = [:]) async throws -> JSONValue {
        guard let engine else { throw ErrorBody(code: "closed", message: String(localized: "The engine isn't running.")) }
        return try await engine.request(method, params)
    }
}

/// What a page asks before it does something: a title, the words, and the button.
struct AdminOffer: Identifiable {
    let id = UUID()
    var title: String
    var text: String
    var ok: String
    var danger = false
    var run: @MainActor () async -> Void
}

extension View {
    /// The pieces every admin page shares: one alert for a failure, a note or a question (`admin.offer`),
    /// and the sign-in form in the page's place when the session ends (the server forgets one after two
    /// idle hours or a restart); the pages above the Overview close with it, and signing in shows the Overview.
    func adminPage(_ admin: AdminModel) -> some View {
        modifier(AdminPage(admin: admin))
    }

    /// A page's loading: when it appears and each time `tick` changes, but only while the admin is
    /// signed in (and again once they are: a page under the sign-in form must not ask the server, and
    /// has to load when the form is done).
    func adminTask(_ admin: AdminModel, _ tick: Int = 0, load: @escaping () async -> Void) -> some View {
        task(id: "\(tick) \(admin.signedIn)") { if admin.signedIn { await load() } }
    }
}

private struct AdminPage: ViewModifier {
    let admin: AdminModel

    /// What the alert says: a failure first, then a note, then a question (one alert at a time: several
    /// `.alert`s on one view don't all present).
    private enum Shown {
        case failure(String), info(String), offer(AdminOffer)

        var title: String {
            switch self {
            case .failure: String(localized: "Something went wrong")
            case .info(let text): text
            case .offer(let o): o.title
            }
        }
    }

    private var shown: Shown? {
        if let f = admin.failure { return .failure(f) }
        if let i = admin.info { return .info(i) }
        if let o = admin.offer { return .offer(o) }
        return nil
    }

    func body(content: Content) -> some View {
        Group {
            if admin.signedIn {
                content
            } else {
                AdminSignIn(admin: admin).navigationTitle("Server Administration")
            }
        }
            .alert(
                shown?.title ?? "",
                isPresented: Binding(
                    get: { shown != nil },
                    set: { if !$0 { clear() } }
                ),
                presenting: shown
            ) { s in
                if case .offer(let o) = s {
                    Button(o.ok, role: o.danger ? .destructive : nil) { Task { await o.run() } }
                    Button("Cancel", role: .cancel) {}
                } else {
                    Button("OK", role: .cancel) {}
                }
            } message: { s in
                if case .failure(let text) = s { Text(text) }
                if case .offer(let o) = s { Text(o.text) }
            }
    }

    /// The alert was answered: whatever it showed is done with.
    private func clear() {
        if admin.failure != nil { admin.failure = nil } else if admin.info != nil { admin.info = nil } else { admin.offer = nil }
    }
}

/// Numbers, sizes and times as the web console writes them.
enum AdminFormat {
    /// Decimal units like the console's (the server's sizes are bytes); a size the server is still
    /// counting (`null`) is "…".
    static func bytes(_ v: JSONValue) -> String {
        guard let n = v.int else { return "…" }
        return n.formatted(.byteCount(style: .file, spellsOutZero: false))
    }

    static func number(_ v: JSONValue) -> String {
        (v.int ?? 0).formatted()
    }

    /// "3 minutes ago" for a time in unix seconds.
    static func ago(_ secs: Int64) -> String {
        RelativeDateTimeFormatter().localizedString(for: Date(timeIntervalSince1970: Double(secs)), relativeTo: .now)
    }

    /// "40 s", "12 min", "2 h 5 min".
    static func duration(_ secs: Int64) -> String {
        if secs < 60 { return String(localized: "\(secs) s") }
        let m = Int((Double(secs) / 60).rounded())
        return m < 60 ? String(localized: "\(m) min") : String(localized: "\(m / 60) h \(m % 60) min")
    }

    /// What a user's storage is made of, when they have library folders (`bytes` counts both).
    static func storageParts(_ u: JSONValue) -> String? {
        guard let total = u["bytes"].int, let folders = u["folderBytes"].int, folders > 0 else { return nil }
        return String(localized: "\(bytes(.int(total - folders))) kept by LightCraft · \(bytes(.int(folders))) in library folders")
    }

    /// A user name the server accepts: letters, digits, `.`, `-`, `_`, up to 64, not starting with a dot.
    static func isUserName(_ s: String) -> Bool {
        !s.isEmpty && s.count <= 64 && !s.hasPrefix(".") && s.allSatisfy { $0.isASCII && ($0.isLetter || $0.isNumber || "._-".contains($0)) }
    }
}

/// A user's initial in a coloured circle (one of eight, by name, as the console's).
struct AdminAvatar: View {
    let name: String
    var size: CGFloat = 34

    var body: some View {
        var h = 0
        for c in name.unicodeScalars { h = (h * 31 + Int(c.value)) % 997 }
        let palette: [Color] = [.blue, .teal, .green, .orange, .red, .pink, .purple, .indigo]
        return Text(String(name.prefix(1)).uppercased())
            .font(.system(size: size * 0.45, weight: .semibold))
            .foregroundStyle(.white)
            .frame(width: size, height: size)
            .background(palette[h % palette.count].gradient, in: Circle())
            .accessibilityHidden(true)
    }
}

/// How full something is: orange past three quarters, red past nine tenths (the console's meters).
struct AdminMeter: View {
    let fraction: Double
    var warns = true

    var body: some View {
        let f = min(max(fraction.isFinite ? fraction : 0, 0), 1)
        ProgressView(value: f).tint(warns && f > 0.9 ? .red : warns && f > 0.75 ? .orange : .accentColor)
    }
}
#endif
