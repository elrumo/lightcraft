import LightCraftCore
import SwiftUI

/// Server Administration ▸ Search & People: what the server does with the users' photos so their
/// devices don't have to: find them by description, by the words printed in them and by the people in
/// them. Each needs models, downloaded once onto the server (the licence first, then progress here);
/// People is switched on per user, because faces are personal data.
struct AdminSearch: View {
    let admin: AdminModel
    @State private var search: JSONValue = .null
    @State private var tick = 0

    /// What each switch says about itself (the console's words).
    private enum Feature {
        case description, words

        var path: [String] { self == .description ? ["search"] : ["search", "text"] }
        func pick(_ s: JSONValue) -> JSONValue { self == .description ? s : s["text"] }
        /// The per-user count that shows how far the work is.
        var done: String { self == .description ? "indexed" : "read" }

        var title: LocalizedStringKey { self == .description ? "Search by Description" : "Words in Photos" }
        var label: LocalizedStringKey { self == .description ? "Search for everyone on this server" : "Find photos by the text printed in them" }

        func help(_ f: JSONValue) -> String {
            let size = AdminFormat.bytes(f["sizeBytes"])
            if f["download"]["running"].bool == true {
                return self == .description
                    ? String(localized: "Downloading the search model onto this server…")
                    : String(localized: "Downloading the text-reading models onto this server…")
            }
            if f["active"].bool == true {
                return self == .description
                    ? String(localized: "On: every device signed in to this server searches here.")
                    : String(localized: "On: the server reads the words in every user's photos in the background, and searches find photos by them.")
            }
            if f["installed"].bool == true {
                return self == .description
                    ? String(localized: "Off. The model stays on this server, so turning this on again needs no download.")
                    : String(localized: "Off. What was read is kept, so turning this on again picks up where it left off.")
            }
            return self == .description
                ? String(localized: "Needs the search model: \(size), downloaded once onto this server.")
                : String(localized: "Needs the text-reading models: \(size), downloaded once onto this server.")
        }

        /// What the users list says when nobody has progress to show.
        func empty(_ f: JSONValue) -> String {
            if f["active"].bool == true { return String(localized: "No users yet.") }
            if f["download"]["running"].bool == true {
                return self == .description ? String(localized: "Photos are described once the model is ready.") : String(localized: "Photos are read once the models are ready.")
            }
            return self == .description ? String(localized: "Nothing is described while search is off.") : String(localized: "Nothing is read while this is off.")
        }

        var offerTitle: String {
            self == .description ? String(localized: "Download the search model?") : String(localized: "Download the text-reading models?")
        }

        func offerText(_ f: JSONValue) -> String {
            let size = AdminFormat.bytes(f["sizeBytes"])
            let licence = f["license"].string ?? "", url = f["licenseUrl"].string ?? ""
            return self == .description
                ? String(localized: "Search by description uses an AI model that understands photos and words. It is not part of LightCraft: it is SigLIP 2 from Google, a \(size) download onto this server under the \(licence). Downloading it means accepting that licence: \(url) Afterwards the server describes every user's photos in the background, and their devices search through it.")
                : String(localized: "Finding words in photos uses two small AI models that read text, in English, Chinese, Japanese and many other languages. They are not part of LightCraft: they are PP-OCRv6 from Baidu, a \(size) download onto this server under the \(licence). Downloading them means accepting that licence: \(url) Afterwards the server reads every user's photos in the background, about a second each.")
        }
    }

    var body: some View {
        List {
            Section {
                Text("What the server does with your users' photos so their devices don't have to: find them by description, by the words printed in them, and by the people in them. Each needs a model, downloaded once onto this server.")
                    .foregroundStyle(.secondary)
            }
            if search.isNull {
                Section { ProgressView().frame(maxWidth: .infinity) }
            } else {
                feature(.description)
                feature(.words)
                people
            }
        }
        .navigationTitle("Search & People")
        .inlineTitle()
        .adminPage(admin)
        .adminTask(admin, tick) { await load() }
        .refreshable { await load() }
    }

    // MARK: description and words

    @ViewBuilder private func feature(_ feat: Feature) -> some View {
        let f = feat.pick(search)
        let d = f["download"]
        let installed = f["installed"].bool == true
        Section {
            Toggle(feat.label, isOn: Binding(
                get: { f["enabled"].bool == true && (installed || d["running"].bool == true) },
                set: { toggle(feat, $0) }
            ))
            Text(feat.help(f)).font(.footnote).foregroundStyle(.secondary)
            fetch(d, path: feat.path)
            failed(d, installed: installed)
            if feat == .description {
                LabeledContent("Model", value: (search["model"].string ?? "") + (installed ? "" : String(localized: " (not downloaded)")))
                licence(f)
                LabeledContent("Kept in") { Text(search["dir"].string ?? "").font(.footnote.monospaced()).textSelection(.enabled) }
                if f["active"].bool == true {
                    LabeledContent("In memory", value: search["loaded"].bool == true
                        ? String(localized: "Yes (it unloads after ten idle minutes)")
                        : String(localized: "Not now (it loads on the first search)"))
                }
            } else {
                LabeledContent("Models", value: installed ? String(localized: "PP-OCRv6 (installed)") : String(localized: "PP-OCRv6 (not downloaded)"))
                licence(f)
            }
        } header: {
            Text(feat.title)
        }
        let rows = (search["users"].array ?? []).filter { !$0[feat.done].isNull }
        Section {
            if rows.isEmpty {
                Text(feat.empty(f)).foregroundStyle(.secondary)
            }
            ForEach(rows, id: \.self) { u in
                let done = u[feat.done].int ?? 0, total = u["total"].int ?? 0
                VStack(alignment: .leading, spacing: 6) {
                    HStack {
                        AdminAvatar(name: u["name"].string ?? "", size: 26)
                        Text(u["name"].string ?? "")
                        Spacer()
                        Text(verbatim: "\(done.formatted()) / \(total.formatted())").font(.caption).foregroundStyle(.secondary)
                    }
                    ProgressView(value: total > 0 ? min(Double(done) / Double(total), 1) : 1)
                }
            }
        }
    }

    @ViewBuilder private func licence(_ f: JSONValue) -> some View {
        if let name = f["license"].string {
            LabeledContent("Licence") {
                if let url = f["licenseUrl"].string.flatMap(URL.init(string:)) {
                    Link(name, destination: url)
                } else {
                    Text(name)
                }
            }
        }
    }

    /// A download under way: how far, and Cancel.
    @ViewBuilder private func fetch(_ d: JSONValue, path: [String]) -> some View {
        if d["running"].bool == true {
            let total = d["total"].int ?? 0, done = d["done"].int ?? 0
            let file = d["file"].string.flatMap { $0.isEmpty ? nil : " · \($0)" } ?? ""
            VStack(alignment: .leading, spacing: 6) {
                ProgressView(value: total > 0 ? min(Double(done) / Double(total), 1) : 0)
                Text(verbatim: "\(AdminFormat.bytes(.int(done))) of \(AdminFormat.bytes(.int(total)))\(file)").font(.caption).foregroundStyle(.secondary)
                Button("Cancel Download", role: .destructive) {
                    Task {
                        await admin.attempt { try await admin.call("DELETE", path + ["download"]) }
                        tick += 1
                    }
                }
            }
        }
    }

    /// A download that failed (one the admin cancelled is not an error).
    @ViewBuilder private func failed(_ d: JSONValue, installed: Bool) -> some View {
        if let why = d["error"].string, d["running"].bool != true, !installed, !why.lowercased().contains("cancel") {
            VStack(alignment: .leading, spacing: 2) {
                Text("The download didn't finish").font(.subheadline.weight(.semibold))
                Text("\(why) Start it again to resume where it stopped.").font(.caption)
            }
            .foregroundStyle(.red)
        }
    }

    /// Turn a feature on or off. Turning it on without its models asks first (the licence), then downloads them.
    private func toggle(_ feat: Feature, _ want: Bool) {
        let known = feat.pick(search)
        if want && known["installed"].bool != true {
            // (what the page shows says the models aren't there: ask at once, as the console does after looking again)
            admin.offer = downloadOffer(feat, known)
            return
        }
        Task {
            await admin.attempt {
                let now = feat.pick(try await admin.get("search"))
                if !want {
                    if now["download"]["running"].bool == true { try await admin.call("DELETE", feat.path + ["download"]) }
                    try await admin.call("PUT", feat.path, ["enabled": false])
                } else if now["installed"].bool == true {
                    try await admin.call("PUT", feat.path, ["enabled": true])
                } else {
                    admin.offer = downloadOffer(feat, now)
                }
            }
            tick += 1
        }
    }

    private func downloadOffer(_ feat: Feature, _ f: JSONValue) -> AdminOffer {
        AdminOffer(title: feat.offerTitle, text: feat.offerText(f), ok: String(localized: "Download and Turn On")) {
            await admin.attempt { try await admin.call("POST", feat.path + ["download"], ["acknowledged": true]) }
            tick += 1
        }
    }

    // MARK: people

    @ViewBuilder private var people: some View {
        let f = search["faces"]
        let d = f["download"]
        let installed = f["installed"].bool == true
        Section {
            if !installed {
                Text(d["running"].bool == true
                    ? String(localized: "Downloading the face models onto this server…")
                    : String(localized: "Needs the face models: \(AdminFormat.bytes(f["sizeBytes"])), downloaded once onto this server."))
                    .font(.footnote).foregroundStyle(.secondary)
                fetch(d, path: ["search", "faces"])
                failed(d, installed: installed)
                if d["running"].bool != true {
                    Button("Download the Face Models…", systemImage: "sparkles") { admin.offer = downloadFaces(f) }
                }
            } else {
                let users = search["users"].array ?? []
                if users.isEmpty { Text("No users yet.").foregroundStyle(.secondary) }
                ForEach(users, id: \.self) { u in
                    let name = u["name"].string ?? ""
                    Toggle(isOn: Binding(get: { u["faces"].bool == true }, set: { togglePeople(u, $0) })) {
                        HStack(spacing: 10) {
                            AdminAvatar(name: name, size: 28)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(name)
                                Text(peopleStatus(u)).font(.caption).foregroundStyle(.secondary)
                            }
                        }
                    }
                }
            }
        } header: {
            Text("People")
        } footer: {
            Text("The server can find the faces in a user's photos and group them into people, which they name on their own devices. Faces are personal data, so this is off for everyone until you turn it on for them, one person at a time. They can delete everything found, from their devices.")
        }
    }

    private func peopleStatus(_ u: JSONValue) -> String {
        guard u["faces"].bool == true else { return String(localized: "Off") }
        guard u["found"].int != nil else { return String(localized: "Waiting for the server…") }
        return String(localized: "\(AdminFormat.number(u["found"])) of \(AdminFormat.number(u["total"])) photos looked at")
    }

    private func downloadFaces(_ f: JSONValue) -> AdminOffer {
        let licences = (f["licenses"].array ?? []).map { String(localized: "the \($0["name"].string ?? "") (\($0["url"].string ?? ""))") }
            .joined(separator: String(localized: " and "))
        let size = AdminFormat.bytes(f["sizeBytes"])
        return AdminOffer(
            title: String(localized: "Download the face models?"),
            text: String(localized: "Finding people uses two small AI models: YuNet finds faces and SFace tells them apart. They are not part of LightCraft. Together they are a \(size) download onto this server, under \(licences). Downloading them means accepting those licences. Nothing is looked at until you turn it on for a user."),
            ok: String(localized: "Download")
        ) {
            await admin.attempt { try await admin.call("POST", ["search", "faces", "download"], ["acknowledged": true]) }
            tick += 1
        }
    }

    /// Finding the people in a user's photos: turning it on asks first, because faces are personal data.
    private func togglePeople(_ u: JSONValue, _ want: Bool) {
        let name = u["name"].string ?? ""
        guard want else {
            Task {
                await admin.attempt { try await admin.call("PUT", ["users", name, "faces"], ["enabled": false]) }
                tick += 1
            }
            return
        }
        admin.offer = AdminOffer(
            title: String(localized: "Find the people in \(name)'s photos?"),
            text: String(localized: "The server will look at the faces in \(name)'s photos and group them into people, which they name on their own devices. What it finds stays on this server, and \(name) can delete all of it from their devices (Forget All Faces). Only do this if they agree."),
            ok: String(localized: "Turn On")
        ) {
            await admin.attempt { try await admin.call("PUT", ["users", name, "faces"], ["enabled": true]) }
            tick += 1
        }
    }

    // MARK: loading

    /// The search state, and again every 2 s while a download or the describing, reading or face work is going on.
    private func load() async {
        while !Task.isCancelled {
            guard await admin.attempt({ search = try await admin.get("search") }) else { return }
            guard working else { return }
            try? await Task.sleep(for: .seconds(2))
        }
    }

    private var working: Bool {
        let users = search["users"].array ?? []
        let features = [Feature.description, .words].contains { feat in
            let f = feat.pick(search)
            return f["download"]["running"].bool == true
                || (f["active"].bool == true && users.contains { u in
                    guard let done = u[feat.done].int else { return false }
                    return done < (u["total"].int ?? 0)
                })
        }
        let faces = search["faces"]["download"]["running"].bool == true
            || users.contains { u in
                guard u["faces"].bool == true, let found = u["found"].int else { return false }
                return found < (u["total"].int ?? 0)
            }
        return features || faces
    }
}
