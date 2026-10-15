import LightCraftCore
import SwiftUI
import UniformTypeIdentifiers

/// Import Presets: what the picked files hold, read without adding anything
/// (`preset.importReview`: the presets by group with what they set and what has no counterpart
/// here, LUTs that become profiles, the files that couldn't be read); Import adds them
/// (`preset.importRun`) and the page says how it went.
struct PresetImportSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let review: JSONValue
    @State private var outcome: JSONValue?
    @State private var working = false

    var body: some View {
        let items = review["items"].array ?? []
        let profiles = review["profiles"].array ?? []
        NavigationStack {
            List {
                if let o = outcome {
                    done(o)
                } else {
                    Section {
                        Text(summary(items.count, profiles.count)).font(.subheadline).foregroundStyle(.secondary)
                    } header: {
                        Text(review["source"].string ?? "")
                    }
                    ForEach(groups(items), id: \.0) { group, list in
                        Section(group) {
                            ForEach(Array(list.enumerated()), id: \.offset) { _, i in
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(i["preset"]["name"].string ?? "")
                                    Text(Self.sets(i["preset"]["settings"])).font(.caption).foregroundStyle(.secondary)
                                    let left = (i["unmapped"].array ?? []).compactMap(\.string)
                                    if !left.isEmpty {
                                        Text("Not carried over: \(left.joined(separator: ", "))").font(.caption).foregroundStyle(.orange)
                                    }
                                }
                            }
                        }
                    }
                    if !profiles.isEmpty {
                        Section("Profiles") {
                            ForEach(Array(profiles.enumerated()), id: \.offset) { _, p in
                                LabeledContent(p.array?[safe: 0]?.string ?? "", value: p.array?[safe: 1]?.string ?? "")
                            }
                        }
                    }
                    problems(review["problems"])
                }
            }
            .navigationTitle("Import Presets")
            .inlineTitle()
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(outcome == nil ? "Cancel" : "Done") { dismiss() }
                }
                if outcome == nil, !(items.isEmpty && profiles.isEmpty) {
                    ToolbarItem(placement: .confirmationAction) {
                        Button("Import") { run() }.disabled(working)
                    }
                }
            }
        }
    }

    private func summary(_ presets: Int, _ profiles: Int) -> String {
        var parts: [String] = []
        if presets > 0 { parts.append(String(localized: "\(presets) presets to add")) }
        if profiles > 0 { parts.append(String(localized: "\(profiles) profiles to add")) }
        if let already = review["already"].int, already > 0 { parts.append(String(localized: "\(already) already here")) }
        return parts.isEmpty ? String(localized: "Nothing new to add") : parts.joined(separator: " · ")
    }

    /// The presets as they would be filed, by group (A–Z).
    private func groups(_ items: [JSONValue]) -> [(String, [JSONValue])] {
        var out: [String: [JSONValue]] = [:]
        for i in items { out[i["preset"]["group"].string ?? "", default: []].append(i) }
        return out.sorted { $0.key.localizedStandardCompare($1.key) == .orderedAscending }
    }

    @ViewBuilder private func problems(_ list: JSONValue) -> some View {
        let rows = list.array ?? []
        if !rows.isEmpty {
            Section("Couldn't Be Read") {
                ForEach(Array(rows.enumerated()), id: \.offset) { _, p in
                    VStack(alignment: .leading) {
                        Text(p.array?[safe: 0]?.string ?? "")
                        Text(p.array?[safe: 1]?.string ?? "").font(.caption).foregroundStyle(.secondary)
                    }
                }
            }
        }
    }

    /// How it went: what was added and where, what was there already, what was left out.
    @ViewBuilder private func done(_ o: JSONValue) -> some View {
        Section {
            if let e = o["error"].string {
                Label(e, systemImage: "exclamationmark.triangle").foregroundStyle(.orange)
            }
            if let n = o["presets"].int, n > 0 {
                Label("Imported \(n) presets", systemImage: "checkmark.circle")
                ForEach(Array((o["groups"].array ?? []).enumerated()), id: \.offset) { _, g in
                    LabeledContent(g.array?[safe: 0]?.string ?? "", value: "\(g.array?[safe: 1]?.int ?? 0)")
                }
            }
            if let n = o["profiles"].int, n > 0 { Label("Imported \(n) profiles (Profile browser ▸ their groups)", systemImage: "checkmark.circle") }
            if let n = o["skipped"].int, n > 0 { Text("\(n) were here already").foregroundStyle(.secondary) }
            let left = (o["notCarried"].array ?? []).compactMap(\.string)
            if !left.isEmpty { Text("Not carried over: \(left.joined(separator: ", "))").font(.footnote).foregroundStyle(.secondary) }
        }
        problems(o["problems"])
    }

    private func run() {
        working = true
        Task {
            outcome = await model.run("preset.importRun", ["review": review])
            working = false
        }
    }

    /// What a preset's settings touch, as the edit groups name them.
    static func sets(_ settings: JSONValue) -> String {
        let names: [(String, String)] = [
            ("profile", "Profile"), ("wb", "White Balance"), ("light", "Light"), ("curve", "Tone Curve"), ("color", "Color"),
            ("mixer", "Color Mixer"), ("grading", "Color Grading"), ("effects", "Effects"), ("vignette", "Vignette"), ("grain", "Grain"),
            ("detail", "Detail"), ("optics", "Optics"), ("geometry", "Geometry"), ("crop", "Crop"), ("masks", "Masks"),
        ]
        return names.filter { !settings[$0.0].isNull }.map { String(localized: String.LocalizationValue($0.1)) }.joined(separator: ", ")
    }
}

/// The files a preset import takes.
enum PresetFiles {
    static let types: [UTType] = ["lcpreset", "xmp", "lrtemplate", "zip", "dng", "lmp", "mplumpack", "cube"].compactMap { UTType(filenameExtension: $0) }

    /// Copy picked files in (they may be another app's), for the engine to read.
    static func stage(_ urls: [URL]) -> [String] {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("PresetImport", isDirectory: true)
        try? FileManager.default.removeItem(at: dir)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return urls.compactMap { url in
            let scoped = url.startAccessingSecurityScopedResource()
            defer { if scoped { url.stopAccessingSecurityScopedResource() } }
            let copy = dir.appendingPathComponent(url.lastPathComponent)
            return (try? FileManager.default.copyItem(at: url, to: copy)) != nil ? copy.path : nil
        }
    }
}
