#if os(iOS)
import LightCraftCore
import SwiftUI

/// Filter: stars (at least, exactly, at most), flags, colour labels, then lists to pick a type, a
/// camera, a person, a place, a keyword or edited / unedited from. Every change narrows the grid at
/// once (`library.filter`); Clear takes it all off.
struct FilterSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    ratingRow
                    flagRow
                    labelRow
                } footer: {
                    Text("\(model.library.count) photos")
                }
                Section {
                    NavigationLink {
                        FacetList(title: "Type", field: "kind", options: Self.kinds)
                    } label: {
                        valueRow("Type", "photo", current: kindLabel)
                    }
                    NavigationLink {
                        FacetList(title: "Edited", field: "edited", options: [("Edited", .bool(true)), ("Unedited", .bool(false))])
                    } label: {
                        valueRow("Edited", "slider.horizontal.3", current: editedLabel)
                    }
                    NavigationLink {
                        FacetList(title: "Camera", field: "camera", facet: "camera")
                    } label: {
                        valueRow("Camera", "camera", current: filter["camera"].string)
                    }
                    NavigationLink {
                        FacetList(title: "People", field: "person", facet: "person")
                    } label: {
                        valueRow("People", "person", current: filter["person"].string)
                    }
                    NavigationLink {
                        FacetList(title: "Location", field: "place", facet: "place")
                    } label: {
                        valueRow("Location", "mappin.and.ellipse", current: filter["place"].string)
                    }
                    NavigationLink {
                        FacetList(title: "Keyword", field: "keyword", facet: "keyword")
                    } label: {
                        valueRow("Keyword", "tag", current: filter["keyword"].string)
                    }
                }
            }
            .navigationTitle("Filter")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close", systemImage: "xmark") { dismiss() }
                }
                ToolbarItem(placement: .primaryAction) {
                    if model.library.state?.isFiltered == true {
                        Button("Clear") { model.perform("library.clearFilter") }
                    }
                }
            }
        }
        .presentationDetents([.medium, .large])
    }

    private var filter: JSONValue { model.library.state?.filter ?? .null }

    private static let kinds: [(LocalizedStringKey, JSONValue)] = [
        ("Photos", ["kind": "image"]), ("Raw", ["kind": "raw"]), ("Videos", ["kind": "video"]),
        ("HDR", ["merged": "hdr"]), ("Panoramas", ["merged": "panorama"]), ("HDR Panoramas", ["merged": "hdrPanorama"]),
    ]

    private var kindLabel: String? {
        if let k = filter["kind"].string { return k == "image" ? String(localized: "Photos") : k.capitalized }
        if let m = filter["merged"].string { return m == "hdr" ? "HDR" : m.capitalized }
        return nil
    }

    private var editedLabel: String? {
        switch filter["edited"] {
        case .bool(true): String(localized: "Edited")
        case .bool(false): String(localized: "Unedited")
        default: nil
        }
    }

    private func valueRow(_ title: LocalizedStringKey, _ symbol: String, current: String?) -> some View {
        LabeledContent {
            Text(current ?? String(localized: "Any")).foregroundStyle(current == nil ? .secondary : Color.accentColor)
        } label: {
            Label(title, systemImage: symbol)
        }
    }

    /// ≥ / = / ≤ and five stars; tapping the chosen star clears it.
    private var ratingRow: some View {
        let rating = Int(filter["rating"].int ?? 0)
        let op = filter["ratingOp"].string ?? "atLeast"
        return HStack(spacing: 6) {
            Button {
                let next = ["atLeast": "exactly", "exactly": "atMost", "atMost": "atLeast"][op] ?? "atLeast"
                model.perform("library.filter", ["ratingOp": .string(next)])
            } label: {
                Text(op == "atLeast" ? "≥" : op == "exactly" ? "=" : "≤")
                    .font(.title3.weight(.semibold))
                    .frame(width: 32)
            }
            .buttonStyle(.borderless)
            .accessibilityLabel("Rating comparison")
            ForEach(1...5, id: \.self) { star in
                Button {
                    model.perform("library.filter", ["rating": .int(Int64(star == rating ? 0 : star))])
                } label: {
                    Image(systemName: star <= rating ? "star.fill" : "star")
                        .font(.title2)
                        .foregroundStyle(star <= rating ? Color.accentColor : .secondary)
                        .symbolEffect(.bounce, value: rating)
                }
                .buttonStyle(.borderless)
                .accessibilityLabel("\(star) stars")
            }
            Spacer()
        }
    }

    private var flagRow: some View {
        let flag = filter["flag"].string
        return HStack(spacing: 18) {
            ForEach([("pick", "flag.fill", "Picked"), ("none", "flag", "Unflagged"), ("reject", "flag.slash.fill", "Rejected")], id: \.0) { value, symbol, label in
                Button {
                    model.perform("library.filter", ["flag": flag == value ? .null : .string(value)])
                } label: {
                    Image(systemName: symbol)
                        .font(.title2)
                        .foregroundStyle(flag == value ? Color.accentColor : .secondary)
                }
                .buttonStyle(.borderless)
                .accessibilityLabel(LocalizedStringKey(label))
                .accessibilityAddTraits(flag == value ? .isSelected : [])
            }
            Spacer()
        }
    }

    private var labelRow: some View {
        let chosen = Set(filter["labels"].array?.compactMap(\.string) ?? filter["label"].string.map { [$0] } ?? [])
        return HStack(spacing: 14) {
            ForEach(LabelColor.all, id: \.self) { label in
                Button {
                    var next = chosen
                    if next.contains(label) { next.remove(label) } else { next.insert(label) }
                    model.perform("library.filter", ["label": .null, "labels": .array(next.sorted().map { .string($0) })])
                } label: {
                    Circle()
                        .fill(LabelColor.color(label))
                        .frame(width: 26, height: 26)
                        .overlay { if chosen.contains(label) { Image(systemName: "checkmark").font(.caption.bold()).foregroundStyle(.white) } }
                }
                .buttonStyle(.borderless)
                .accessibilityLabel(model.library.labelName(label))
                .accessibilityAddTraits(chosen.contains(label) ? .isSelected : [])
            }
            Spacer()
        }
    }
}

/// One of the filter's lists: Any, then the values (fixed, or the library's own with their counts).
private struct FacetList: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let title: LocalizedStringKey
    /// The filter field it sets.
    let field: String
    var options: [(LocalizedStringKey, JSONValue)] = []
    /// `library.facets` field whose values to list (`camera`, `person`, `place`, `keyword`).
    var facet: String? = nil
    @State private var values: [(String, Int)] = []
    @State private var search = ""

    var body: some View {
        List {
            Button {
                var p: [String: JSONValue] = [field: .null]
                if field == "kind" { p["merged"] = .null }
                model.perform("library.filter", .object(p))
                dismiss()
            } label: {
                row(Text("Any"), chosen: isAny, count: nil)
            }
            ForEach(Array(options.enumerated()), id: \.offset) { _, option in
                Button {
                    var p = option.1
                    if field == "kind" {
                        if p["kind"] == .null { p["kind"] = .null } else { p["merged"] = .null }
                        model.perform("library.filter", p)
                    } else {
                        model.perform("library.filter", [field: p])
                    }
                    dismiss()
                } label: {
                    row(Text(option.0), chosen: chosen(option.1), count: nil)
                }
            }
            ForEach(shown, id: \.0) { value, count in
                Button {
                    model.perform("library.filter", [field: .string(value)])
                    dismiss()
                } label: {
                    row(Text(value), chosen: model.library.state?.filter[field].string == value, count: count)
                }
            }
        }
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
        .searchable(text: $search)
        .task { await load() }
    }

    private var shown: [(String, Int)] {
        let q = search.trimmingCharacters(in: .whitespaces)
        return Array(values.filter { q.isEmpty || $0.0.localizedCaseInsensitiveContains(q) }.prefix(300))
    }

    private var isAny: Bool {
        let f = model.library.state?.filter ?? .null
        return f[field] == .null && (field != "kind" || f["merged"] == .null)
    }

    private func chosen(_ v: JSONValue) -> Bool {
        let f = model.library.state?.filter ?? .null
        if field == "kind" {
            if let k = v["kind"].string { return f["kind"].string == k }
            if let m = v["merged"].string { return f["merged"].string == m }
            return false
        }
        return f[field] == v
    }

    private func row(_ label: Text, chosen: Bool, count: Int?) -> some View {
        HStack {
            label.foregroundStyle(.primary)
            Spacer()
            if let count { Text("\(count)").foregroundStyle(.secondary) }
            if chosen { Image(systemName: "checkmark").foregroundStyle(Color.accentColor) }
        }
    }

    private func load() async {
        guard let facet, let engine = model.engine else { return }
        guard let r = try? await engine.execute("library.facets", ["field": .string(facet)]) else { return }
        values = (r.array ?? []).compactMap { v in
            guard let name = v["value"].string else { return nil }
            return (name, Int(v["count"].int ?? 0))
        }
    }
}
#endif
