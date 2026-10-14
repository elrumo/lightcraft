#if os(iOS)
import LightCraftCore
import SwiftUI

/// Settings ▸ Sync ▸ Storage: what the library takes on the server (originals, previews, library
/// folders, the admin's limit, the server's disk) and on this device (`sync.usage`, asked again
/// every few seconds while it shows; the engine asks the server at most every 20 s).
struct SyncStorageView: View {
    @Environment(AppModel.self) private var model
    @State private var usage: JSONValue = .null

    var body: some View {
        Form {
            let server = usage["server"]
            Section {
                if server.isNull {
                    if let e = usage["serverError"].string {
                        Text("The server can't say how much it stores: \(e)").foregroundStyle(.secondary)
                    } else {
                        HStack { Text("Asking the server…").foregroundStyle(.secondary); Spacer(); ProgressView() }
                    }
                } else {
                    files("Originals", server["original"])
                    files("Smart previews", server["smart"])
                    files("Small previews", server["mini"])
                    if (server["folders"]["files"].int ?? 0) > 0 { files("Library folders", server["folders"]) }
                    LabeledContent("Total", value: Self.bytes(usage["serverTotal"]))
                    if let quota = server["quota"].int {
                        LabeledContent("Storage limit") {
                            Text("\(Self.bytes(usage["serverStored"])) of \(Self.bytes(.int(quota))) used")
                                .foregroundStyle(usage["nearLimit"].bool == true ? .orange : .secondary)
                        }
                    }
                    disk("Server disk", server["disk"], ours: usage["serverOnDisk"])
                }
            } header: {
                Text("On the Server")
            } footer: {
                VStack(alignment: .leading, spacing: 6) {
                    if (server["folders"]["files"].int ?? 0) > 0 {
                        Text("Library folders are read where they are on the server and never copied, so LightCraft keeps only their previews. The total counts their photos too: they take that room on the server.")
                    }
                    if usage["nearLimit"].bool == true {
                        Text("Nearly at the storage limit the server's admin set for you. Over it, new photos wait to upload until there is room; nothing is deleted.")
                    }
                }
            }
            let local = usage["local"]
            if !local.isNull {
                Section {
                    LabeledContent("Photos", value: (usage["photos"]["total"].int ?? 0).formatted())
                    if let only = usage["photos"]["onlyPreviewsHere"].int, only > 0 {
                        LabeledContent("Only Previews Here", value: only.formatted())
                    }
                    files("Originals", local["downloaded"])
                    files("Smart previews", local["smart"])
                    files("Small previews", local["mini"])
                    files("Thumbnails", local["thumbnails"])
                    if (local["imported"]["files"].int ?? 0) > 0 { files("Imported copies", local["imported"]) }
                    LabeledContent("Catalog and edits", value: Self.bytes(local["library"]))
                    LabeledContent("Total", value: Self.bytes(usage["localTotal"]))
                    disk("This disk", local["disk"], ours: usage["localTotal"])
                } header: {
                    Text("On This Device")
                } footer: {
                    Text("Originals are the full-size files downloaded from the server. Thumbnails can be deleted at any time; they are drawn again.")
                }
            }
        }
        .navigationTitle("Storage")
        .navigationBarTitleDisplayMode(.inline)
        .task {
            while !Task.isCancelled {
                if let engine = model.engine, let u = try? await engine.execute("sync.usage") { usage = u }
                try? await Task.sleep(for: .seconds(5))
            }
        }
    }

    /// A kind of file: how much room, and how many.
    private func files(_ label: LocalizedStringKey, _ f: JSONValue) -> some View {
        LabeledContent(label) {
            VStack(alignment: .trailing) {
                Text(Self.bytes(f["bytes"]))
                Text("^[\(f["files"].int ?? 0) file](inflect: true)").font(.caption)
            }
        }
    }

    /// A disk: free of total, with what the library takes of it in the bar.
    @ViewBuilder private func disk(_ label: LocalizedStringKey, _ d: JSONValue, ours: JSONValue) -> some View {
        if let total = d["total"].int, let free = d["free"].int, total > 0 {
            VStack(alignment: .leading, spacing: 6) {
                LabeledContent(label, value: String(localized: "\(Self.bytes(.int(free))) free of \(Self.bytes(.int(total)))"))
                ProgressView(value: Double(max(total - free, 0)), total: Double(total))
                    .overlay(alignment: .leading) {
                        // the library's part of what is used
                        GeometryReader { g in
                            Capsule().fill(Color.accentColor)
                                .frame(width: g.size.width * min(Double(ours.int ?? 0) / Double(total), 1), height: 4)
                                .frame(maxHeight: .infinity)
                        }
                    }
                    .tint(.secondary)
            }
        }
    }

    static func bytes(_ v: JSONValue) -> String {
        (v.int ?? 0).formatted(.byteCount(style: .file))
    }
}
#endif
