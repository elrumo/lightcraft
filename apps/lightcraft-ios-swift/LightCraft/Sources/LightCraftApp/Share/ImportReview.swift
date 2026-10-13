#if os(iOS)
import LightCraftCore
import SwiftUI

/// What the import review shows: the files found (`import.scan`) and which are chosen.
struct ImportReviewData: Equatable {
    var candidates: [JSONValue]
    var chosen: Set<Int>

    init(candidates: [JSONValue]) {
        self.candidates = candidates
        // what the library has already isn't chosen
        chosen = Set(candidates.indices.filter { Self.importable(candidates[$0]) && candidates[$0]["duplicate"] == .null })
    }

    static func importable(_ c: JSONValue) -> Bool {
        c["error"] == .null
    }
}

/// Import Photos: the files picked (three across, a check on each), what the library has already
/// marked, and where they go (an album, keywords, a preset); Add moves them into the library.
struct ImportReviewSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var albumChoice: UInt64 = 0
    @State private var newAlbum = ""
    @State private var keywords = ""
    @State private var preset = ""
    @State private var onDeleted = "restore"
    @State private var presets: [PresetInfo] = []

    var body: some View {
        @Bindable var model = model
        NavigationStack {
            if let review = model.importReview {
                List {
                    Section {
                        HStack {
                            Text(summary(review)).font(.subheadline).foregroundStyle(.secondary)
                            Spacer()
                            Button(review.chosen.count == importableCount(review) ? "Deselect All" : "Select All") {
                                if review.chosen.count == importableCount(review) {
                                    model.importReview?.chosen = []
                                } else {
                                    model.importReview?.chosen = Set(review.candidates.indices.filter { ImportReviewData.importable(review.candidates[$0]) })
                                }
                            }
                            .font(.subheadline)
                        }
                        LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 2), count: 3), spacing: 2) {
                            ForEach(review.candidates.indices, id: \.self) { i in
                                CandidateCell(index: i, candidate: review.candidates[i], chosen: review.chosen.contains(i)) {
                                    guard ImportReviewData.importable(review.candidates[i]) else { return }
                                    if model.importReview?.chosen.contains(i) == true {
                                        model.importReview?.chosen.remove(i)
                                    } else {
                                        model.importReview?.chosen.insert(i)
                                    }
                                }
                            }
                        }
                        .listRowInsets(EdgeInsets())
                    }
                    Section {
                        Picker("Album", selection: $albumChoice) {
                            Text("None").tag(UInt64(0))
                            Text("New Album…").tag(UInt64.max)
                            ForEach(model.albums.plain) { Text($0.name).tag($0.id) }
                        }
                        if albumChoice == .max {
                            TextField("Album Name", text: $newAlbum)
                        }
                        TextField("Keywords (comma separated)", text: $keywords)
                            .textInputAutocapitalization(.never)
                        Picker("Preset", selection: $preset) {
                            Text("None").tag("")
                            ForEach(presets) { Text($0.name).tag($0.id) }
                        }
                        if review.candidates.contains(where: { $0["inTrash"].bool == true }) {
                            Picker("In Recently Deleted", selection: $onDeleted) {
                                Text("Restore").tag("restore")
                                Text("Import as New").tag("fresh")
                                Text("Leave There").tag("skip")
                            }
                        }
                    } header: {
                        Text("Options")
                    } footer: {
                        Text("The photos are copied into the library; the originals in Photos and Files are left as they are.")
                    }
                }
                .navigationTitle("Import Photos")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("Cancel", systemImage: "xmark") {
                            model.importReview = nil
                            dismiss()
                        }
                    }
                    ToolbarItem(placement: .confirmationAction) {
                        Button("Add \(review.chosen.count)") { start(review) }
                            .disabled(review.chosen.isEmpty)
                    }
                }
                .task {
                    guard let engine = model.engine else { return }
                    presets = (try? await engine.execute("presets.list", [:], as: [PresetInfo].self)) ?? []
                    if let album = model.plainAlbum { albumChoice = album }
                }
            } else {
                ProgressView("Looking for photos…")
            }
        }
        .interactiveDismissDisabled(model.importReview != nil)
    }

    private func importableCount(_ review: ImportReviewData) -> Int {
        review.candidates.filter(ImportReviewData.importable).count
    }

    private func summary(_ review: ImportReviewData) -> String {
        let dupes = review.candidates.filter { $0["duplicate"] != .null }.count
        var s = String(localized: "\(review.candidates.count) found · \(review.chosen.count) selected")
        if dupes > 0 { s += " · " + String(localized: "\(dupes) already in the library") }
        return s
    }

    private func start(_ review: ImportReviewData) {
        var p: [String: JSONValue] = ["indexes": .array(review.chosen.sorted().map { .int(Int64($0)) }), "mode": "move", "onDeleted": .string(onDeleted)]
        if albumChoice == .max, !newAlbum.trimmingCharacters(in: .whitespaces).isEmpty {
            p["albumName"] = .string(newAlbum)
        } else if albumChoice != 0, albumChoice != .max {
            p["album"] = .int(Int64(clamping: albumChoice))
        }
        let kw = keywords.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
        if !kw.isEmpty { p["keywords"] = .array(kw.map { .string($0) }) }
        if !preset.isEmpty { p["preset"] = .string(preset) }
        model.importReview = nil
        dismiss()
        Task { await model.importChosen(.object(p)) }
    }
}

/// A file in the review: its thumbnail, a check, and what the library knows of it.
private struct CandidateCell: View {
    @Environment(AppModel.self) private var model
    let index: Int
    let candidate: JSONValue
    let chosen: Bool
    let toggle: () -> Void

    var body: some View {
        Color(uiColor: .secondarySystemBackground)
            .aspectRatio(1, contentMode: .fit)
            .overlay {
                if let p = model.images.pictures["import:\(index)"] {
                    Image(decorative: p.image, scale: 1).resizable().scaledToFill()
                }
            }
            .overlay { if !chosen { Color.black.opacity(0.35) } }
            .overlay(alignment: .topLeading) {
                if let tag {
                    Text(tag)
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(.white)
                        .padding(.horizontal, 6).padding(.vertical, 2)
                        .background(.black.opacity(0.6), in: .capsule)
                        .padding(4)
                }
            }
            .overlay(alignment: .bottomTrailing) {
                if ImportReviewData.importable(candidate) {
                    Image(systemName: chosen ? "checkmark.circle.fill" : "circle")
                        .font(.title3)
                        .symbolRenderingMode(.palette)
                        .foregroundStyle(.white, chosen ? Color.accentColor : .black.opacity(0.3))
                        .padding(5)
                }
            }
            .clipped()
            .contentShape(Rectangle())
            .onTapGesture(perform: toggle)
            .task { model.engine?.send("render.candidate", ["index": .int(Int64(index)), "edge": 256]) }
            .accessibilityElement()
            .accessibilityLabel(candidate["name"].string ?? "")
            .accessibilityValue(tag ?? "")
            .accessibilityAddTraits(chosen ? [.isButton, .isSelected] : .isButton)
    }

    private var tag: String? {
        if candidate["error"] != .null { return String(localized: "Unreadable") }
        if candidate["inTrash"].bool == true { return String(localized: "In Recently Deleted") }
        if let d = candidate["duplicate"].string { return d == "path" ? String(localized: "In Library") : String(localized: "Duplicate") }
        return nil
    }
}
#endif
