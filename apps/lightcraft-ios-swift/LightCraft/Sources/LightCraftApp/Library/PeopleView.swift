#if os(iOS)
import LightCraftCore
import MapKit
import SwiftUI

/// People: the people named in the library (`people.named`), the groups found on this device
/// (`people.list`, desktops), and the faces nobody has named yet (`faces.unnamed`). A named person
/// opens their page; a group or a face asks who it is.
struct PeopleView: View {
    @Environment(AppModel.self) private var model
    @State private var named: [JSONValue] = []
    @State private var found: [JSONValue] = []
    @State private var unnamed: [JSONValue] = []
    @State private var loaded = false
    @State private var naming: FaceNaming?
    @State private var cluster: JSONValue?
    @State private var name = ""

    var body: some View {
        ScrollView {
            if let f = model.state.faces, f.pendingPhotos > 0 {
                Label("Looking for faces in \(f.pendingPhotos) photos", systemImage: "person.crop.rectangle.stack")
                    .font(.footnote).foregroundStyle(.secondary)
                    .padding(.top, 8)
            }
            if loaded && named.isEmpty && found.isEmpty && unnamed.isEmpty {
                ContentUnavailableView {
                    Label("No People Yet", systemImage: "person.2")
                } description: {
                    Text("Faces named by other apps show up here. Turn on finding people in Settings ▸ Faces, or open a photo and tap Tag a Face in its … menu.")
                } actions: {
                    Button("Faces Settings") { model.sheet = .settings("faces") }
                        .buttonStyle(.borderedProminent)
                }
                .padding(.top, 60)
            } else {
                VStack(alignment: .leading, spacing: 8) {
                    if !named.isEmpty {
                        heading("People")
                        LazyVGrid(columns: Self.columns(100), spacing: 20) {
                            ForEach(Array(named.enumerated()), id: \.offset) { _, p in
                                let n = p["name"].string ?? ""
                                Button { model.path.append(.person(n)) } label: {
                                    tile(p["cover"], title: Text(n), sub: Text("^[\(p["photos"].int ?? 0) photo](inflect: true)"))
                                }
                                .buttonStyle(.plain)
                            }
                        }
                    }
                    if !found.isEmpty {
                        heading("Found in Your Photos")
                        LazyVGrid(columns: Self.columns(100), spacing: 20) {
                            ForEach(Array(found.enumerated()), id: \.offset) { _, p in
                                Button { tapped(p) } label: {
                                    let n = p["name"].string
                                    let pending = p["pending"].int ?? 0
                                    tile(p["cover"], title: Text(n ?? String(localized: "Add Name")).foregroundStyle(n == nil ? Color.accentColor : .primary),
                                         sub: n != nil && pending > 0 ? Text("Confirm \(pending) more") : Text("^[\(p["photos"].int ?? 0) photo](inflect: true)"))
                                }
                                .buttonStyle(.plain)
                            }
                        }
                    }
                    if !unnamed.isEmpty {
                        heading("Unnamed Faces")
                        LazyVGrid(columns: Self.columns(72), spacing: 14) {
                            ForEach(Array(unnamed.enumerated()), id: \.offset) { _, f in
                                let guess = f["suggestion"]["name"].string
                                Button {
                                    guard let photo = f["photo"].int, let index = f["index"].int else { return }
                                    naming = FaceNaming(photo: UInt64(clamping: photo), index: Int(index), rect: nil, start: guess ?? "",
                                                        candidates: guess.map { [$0] } ?? [], crop: nil)
                                } label: {
                                    VStack(spacing: 4) {
                                        FaceDisc(photo: f["photo"].int.map { UInt64(clamping: $0) }, rect: f["rect"]).aspectRatio(1, contentMode: .fit)
                                        Text(guess.map { "\($0)?" } ?? " ").font(.caption).foregroundStyle(.secondary).lineLimit(1)
                                    }
                                }
                                .buttonStyle(.plain)
                            }
                        }
                    }
                }
                .padding(16)
            }
        }
        .navigationTitle("People")
        .task(id: model.state.catalogRevision) { await load() }
        .sheet(item: $naming) { FaceNameSheet(target: $0) }
        .alert("Who Is This?", isPresented: Binding(get: { cluster != nil }, set: { if !$0 { cluster = nil } })) {
            TextField("Name", text: $name)
            Button("Done") {
                if let p = cluster, !name.isEmpty { model.perform("people.name", ["cluster": p["id"], "name": .string(name)]) }
                cluster = nil
            }
            Button("Cancel", role: .cancel) { cluster = nil }
        }
    }

    static func columns(_ minimum: CGFloat) -> [GridItem] { [GridItem(.adaptive(minimum: minimum), spacing: 16)] }

    private func heading(_ title: LocalizedStringKey) -> some View {
        Text(title).font(.title3.bold()).padding(.top, 12)
    }

    /// A person or group: their face, the name, a count.
    private func tile(_ cover: JSONValue, title: Text, sub: Text) -> some View {
        VStack(spacing: 4) {
            FaceDisc(photo: cover["photo"].int.map { UInt64(clamping: $0) }, rect: cover["rect"]).aspectRatio(1, contentMode: .fit)
            title.font(.subheadline.weight(.semibold)).lineLimit(1)
            sub.font(.caption).foregroundStyle(.secondary)
        }
    }

    /// A named group with faces waiting confirms them; an unnamed one asks who it is.
    private func tapped(_ p: JSONValue) {
        if let n = p["name"].string {
            model.perform("people.name", ["cluster": p["id"], "name": .string(n)])
        } else {
            name = ""
            cluster = p
        }
    }

    private func load() async {
        guard let engine = model.engine else { return }
        named = (try? await engine.execute("people.named"))?.array ?? []
        found = (try? await engine.execute("people.list", ["all": true]))?["people"].array?.filter { $0["id"].string != nil } ?? []
        unnamed = (try? await engine.execute("faces.unnamed"))?["faces"].array ?? []
        loaded = true
    }
}

/// A person's page: their face and name (tap to rename everyone with it), Show Photos, their
/// faces (a tap opens the photo), and the faces that look like them, ✓ it is them / ✕ it isn't.
struct PersonView: View {
    @Environment(AppModel.self) private var model
    let name: String
    @State private var current: String?
    @State private var info: JSONValue = .null
    @State private var cover: JSONValue = .null
    @State private var dismissed: Set<String> = []
    @State private var renaming = false
    @State private var newName = ""

    private var shown: String { current ?? name }

    var body: some View {
        let confirmed = info["confirmed"].array ?? []
        let more = (info["more"].array ?? []).filter { !dismissed.contains(Self.key($0)) }
        ScrollView {
            VStack(spacing: 8) {
                FaceDisc(photo: (cover["photo"].int ?? confirmed.first?["photo"].int).map { UInt64(clamping: $0) },
                         rect: cover.isNull ? confirmed.first?["rect"] ?? .null : cover["rect"])
                    .frame(width: 116, height: 116)
                Button {
                    newName = shown
                    renaming = true
                } label: {
                    Text(shown).font(.title.bold())
                }
                .buttonStyle(.plain)
                .accessibilityHint("Rename")
                Text("^[\(cover["photos"].int ?? info["total"].int ?? 0) photo](inflect: true)").foregroundStyle(.secondary)
                Button("Show Photos") {
                    Task {
                        await model.run("library.filter", ["person": .string(shown)])
                        model.prefs.gridSpan = .all
                        model.path = []
                    }
                }
                .buttonStyle(.borderedProminent)
            }
            .frame(maxWidth: .infinity)
            .padding(.top, 12)
            VStack(alignment: .leading, spacing: 8) {
                if !confirmed.isEmpty {
                    Text("Photos").font(.title3.bold()).padding(.top, 16)
                    LazyVGrid(columns: Self.columns, spacing: 6) {
                        ForEach(Array(confirmed.enumerated()), id: \.offset) { _, f in
                            Button {
                                if let p = f["photo"].int { model.open(UInt64(clamping: p)) }
                            } label: {
                                FaceDisc(photo: f["photo"].int.map { UInt64(clamping: $0) }, rect: f["rect"], square: true).aspectRatio(1, contentMode: .fit)
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
                HStack {
                    Text("Is this \(shown)?").font(.title3.bold())
                    Spacer()
                    if more.count > 1 {
                        Button("Confirm All") {
                            model.perform("faces.nameFaces", ["faces": .array(more.map { ["photo": $0["photo"], "index": $0["index"]] }), "name": .string(shown)])
                        }
                        .buttonStyle(.bordered)
                    }
                }
                .padding(.top, 16)
                let note: LocalizedStringKey = more.isEmpty
                    ? (info["ready"].bool == true ? "No other faces look like them yet: more appear as your photos are looked at."
                                                  : "Turn on finding people in Settings ▸ Faces to find more of their photos.")
                    : "Tap ✓ when it is them, ✕ when it isn't."
                Text(note).font(.footnote).foregroundStyle(.secondary)
                LazyVGrid(columns: Self.columns, spacing: 6) {
                    ForEach(Array(more.enumerated()), id: \.offset) { _, f in
                        FaceDisc(photo: f["photo"].int.map { UInt64(clamping: $0) }, rect: f["rect"], square: true)
                            .aspectRatio(1, contentMode: .fit)
                            .overlay(alignment: .bottom) {
                                HStack(spacing: 0) {
                                    Button("It Isn't", systemImage: "xmark") { dismissed.insert(Self.key(f)) }
                                    Button("It Is", systemImage: "checkmark") {
                                        model.perform("faces.setName", ["id": f["photo"], "index": f["index"], "name": .string(shown)])
                                    }
                                }
                                .labelStyle(.iconOnly)
                                .buttonStyle(.plain)
                                .foregroundStyle(.white)
                                .frame(maxWidth: .infinity, minHeight: 30)
                                .background(.black.opacity(0.55))
                            }
                            .clipShape(.rect(cornerRadius: 10))
                    }
                }
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 24)
        }
        .sensoryFeedback(.selection, trigger: confirmed.count)
        .navigationTitle(shown)
        .navigationBarTitleDisplayMode(.inline)
        .task(id: Load(name: shown, revision: model.state.catalogRevision)) {
            guard let engine = model.engine else { return }
            info = (try? await engine.execute("faces.person", ["name": .string(shown)])) ?? .null
            let people = (try? await engine.execute("people.named"))?.array ?? []
            cover = people.first { $0["name"].string?.caseInsensitiveCompare(shown) == .orderedSame } ?? .null
        }
        .alert("Rename", isPresented: $renaming) {
            TextField("Name", text: $newName)
            Button("Rename") { rename() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Every face named \(shown) gets the new name; a name someone already has puts them together.")
        }
    }

    private static let columns = Array(repeating: GridItem(.flexible(), spacing: 6), count: 4)

    private static func key(_ f: JSONValue) -> String { "\(f["photo"].int ?? 0):\(f["index"].int ?? 0)" }

    private func rename() {
        let to = newName.trimmingCharacters(in: .whitespaces)
        guard !to.isEmpty, to != shown else { return }
        Task {
            if await model.run("people.rename", ["from": .string(shown), "to": .string(to)]) != nil { current = to }
        }
    }

    private nonisolated struct Load: Hashable {
        let name: String
        let revision: UInt64
    }
}

/// A person's face in a disc: the face itself (`render.face`), its photo until it comes.
struct FaceDisc: View {
    @Environment(AppModel.self) private var model
    let photo: UInt64?
    /// The face's box, as `people.list` gives it.
    var rect: JSONValue = .null
    /// A rounded square instead of a disc.
    var square = false
    @State private var slot: String?

    var body: some View {
        let shape = square ? AnyShape(.rect(cornerRadius: 10)) : AnyShape(.circle)
        shape
            .fill(Color(uiColor: .tertiarySystemFill))
            .overlay {
                if let slot, let p = model.images.pictures[slot] {
                    Image(decorative: p.image, scale: 1).resizable().scaledToFill()
                } else if let photo, let p = model.images.thumb(photo) {
                    Image(decorative: p.image, scale: 1).resizable().scaledToFill()
                } else {
                    Image(systemName: "person.fill").font(.largeTitle).foregroundStyle(.secondary)
                }
            }
            .clipShape(shape)
            .task(id: Key(photo: photo, rect: rect, revision: model.state.catalogRevision)) {
                guard let photo, let engine = model.engine else { return }
                if rect.isNull {
                    model.wantThumb(photo, edge: 256)
                } else {
                    slot = (try? await engine.request("render.face", ["photo": .int(Int64(clamping: photo)), "rect": rect, "edge": 256]))?["slot"].string
                }
            }
            .onDisappear { if let photo, rect.isNull { model.thumbDisappeared(photo) } }
    }

    private nonisolated struct Key: Hashable {
        let photo: UInt64?
        let rect: JSONValue
        let revision: UInt64
    }
}

/// The map: the grid's photos where they were taken, clustered; a tap on a cluster shows its photos
/// in the grid.
struct MapScreen: View {
    @Environment(AppModel.self) private var model
    @State private var markers: [JSONValue] = []
    @State private var located = 0
    @State private var camera: MapCameraPosition = .automatic
    @State private var zoom: Double = 3
    @State private var search = ""
    @State private var places: [JSONValue] = []

    var body: some View {
        Map(position: $camera) {
            ForEach(Array(markers.enumerated()), id: \.offset) { _, m in
                if let lat = m["lat"].double, let lon = m["lon"].double {
                    Annotation("", coordinate: CLLocationCoordinate2D(latitude: lat, longitude: lon)) {
                        // (the model passed in: MapKit hosts annotations apart, where the environment can be missing)
                        MapMarker(model: model, photos: (m["pictured"].array ?? []).compactMap { $0.int.map { UInt64(clamping: $0) } }, count: Int(m["count"].int ?? 0))
                            .onTapGesture { show(m) }
                            .contextMenu {
                                Button("Show in Grid", systemImage: "square.grid.3x3") { show(m) }
                                Button("Select", systemImage: "checkmark.circle") { select(m) }
                                Button("Zoom to These Photos", systemImage: "plus.magnifyingglass") { go(m["bounds"]) }
                            }
                    }
                }
            }
        }
        .mapStyle(.standard(elevation: .realistic))
        .onMapCameraChange(frequency: .onEnd) { ctx in
            let span = ctx.region.span.longitudeDelta
            zoom = max(0, min(18, log2(360 / max(span, 0.0001))))
            Task { await load(region: ctx.region) }
        }
        .overlay(alignment: .top) {
            if located == 0 {
                Text("No photos with a location")
                    .font(.footnote)
                    .padding(.horizontal, 12).padding(.vertical, 6)
                    .glassEffect(.regular, in: .capsule)
                    .padding(.top, 8)
            }
        }
        .navigationTitle("Map")
        .navigationBarTitleDisplayMode(.inline)
        .searchable(text: $search, prompt: "Go to a place")
        .searchSuggestions {
            ForEach(Array(places.enumerated()), id: \.offset) { _, p in
                Button(p["label"].string ?? "") { go(p) }
            }
        }
        .task(id: search) {
            guard let engine = model.engine, search.count >= 2 else { places = []; return }
            places = (try? await engine.execute("map.place", ["query": .string(search)]))?.array ?? []
        }
        .task {
            guard let engine = model.engine, let b = try? await engine.execute("map.bounds") else { return }
            located = Int(b["located"].int ?? 0)
            if let s = b["south"].double, let w = b["west"].double, let n = b["north"].double, let e = b["east"].double, located > 0 {
                camera = .region(MKCoordinateRegion(
                    center: CLLocationCoordinate2D(latitude: (s + n) / 2, longitude: (w + e) / 2),
                    span: MKCoordinateSpan(latitudeDelta: max(n - s, 0.05) * 1.3, longitudeDelta: max(e - w, 0.05) * 1.3)
                ))
            }
        }
    }

    private func load(region: MKCoordinateRegion) async {
        guard let engine = model.engine else { return }
        let c = region.center, sp = region.span
        let bbox: JSONValue = [.double(c.latitude - sp.latitudeDelta / 2), .double(c.longitude - sp.longitudeDelta / 2), .double(c.latitude + sp.latitudeDelta / 2), .double(c.longitude + sp.longitudeDelta / 2)]
        guard let r = try? await engine.execute("map.points", ["zoom": .double(zoom), "bbox": bbox, "ids": 200]) else { return }
        markers = r["markers"].array ?? []
        located = Int(r["located"].int ?? 0)
    }

    private func show(_ m: JSONValue) {
        guard let ids = m["ids"].array, !ids.isEmpty else { return }
        Task {
            await model.run("library.filter", ["only": .array(ids)])
            model.prefs.gridSpan = .all
            model.path = []
        }
    }

    /// A marker's photos selected (in the grid they are in).
    private func select(_ m: JSONValue) {
        guard let ids = m["ids"].array, !ids.isEmpty else { return }
        model.perform("library.select", ["ids": .array(ids)])
    }

    private func go(_ p: JSONValue) {
        guard let s = p["south"].double, let w = p["west"].double, let n = p["north"].double, let e = p["east"].double else { return }
        withAnimation {
            camera = .region(MKCoordinateRegion(
                center: CLLocationCoordinate2D(latitude: (s + n) / 2, longitude: (w + e) / 2),
                span: MKCoordinateSpan(latitudeDelta: max(n - s, 0.02), longitudeDelta: max(e - w, 0.02))
            ))
        }
        search = ""
    }
}

/// A cluster on the map: up to four of its photos framed in white (`map.points` → `pictured`: one
/// fills it, two side by side, three a tall one and two small, four a square), the count in a badge.
private struct MapMarker: View {
    let model: AppModel
    let photos: [UInt64]
    let count: Int

    var body: some View {
        RoundedRectangle(cornerRadius: 8)
            .fill(.white)
            .frame(width: 52, height: 52)
            .overlay { mosaic.frame(width: 46, height: 46).clipShape(.rect(cornerRadius: 6)) }
            .overlay(alignment: .topTrailing) {
                if count > 1 {
                    Text("\(count)")
                        .font(.caption2.bold())
                        .foregroundStyle(.white)
                        .padding(.horizontal, 6).padding(.vertical, 2)
                        .background(Color.accentColor, in: .capsule)
                        .offset(x: 8, y: -8)
                }
            }
            .shadow(radius: 3)
            .task(id: photos) { for p in photos { model.wantThumb(p, edge: 128) } }
            .onDisappear { for p in photos { model.thumbDisappeared(p) } }
            .accessibilityLabel("\(count) photos")
    }

    @ViewBuilder private var mosaic: some View {
        let gap: CGFloat = 2
        switch photos.count {
        case 0: Color(uiColor: .tertiarySystemFill)
        case 1: thumb(0)
        case 2: HStack(spacing: gap) { thumb(0); thumb(1) }
        case 3: HStack(spacing: gap) { thumb(0); VStack(spacing: gap) { thumb(1); thumb(2) } }
        default:
            VStack(spacing: gap) {
                HStack(spacing: gap) { thumb(0); thumb(1) }
                HStack(spacing: gap) { thumb(2); thumb(3) }
            }
        }
    }

    private func thumb(_ i: Int) -> some View {
        Color(uiColor: .tertiarySystemFill)
            .overlay {
                if let id = photos[safe: i], let p = model.images.thumb(id) {
                    Image(decorative: p.image, scale: 1).resizable().scaledToFill()
                }
            }
            .clipped()
    }
}
#endif
