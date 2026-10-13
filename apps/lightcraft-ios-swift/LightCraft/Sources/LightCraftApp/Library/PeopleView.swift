#if os(iOS)
import LightCraftCore
import MapKit
import SwiftUI

/// People: the faces found in the library, grouped by person; a named person opens their photos,
/// an unnamed group asks who it is. Finding faces runs on this device (Settings ▸ AI Models).
struct PeopleView: View {
    @Environment(AppModel.self) private var model
    @State private var answer: JSONValue = .null
    @State private var naming: JSONValue?
    @State private var name = ""

    var body: some View {
        let people = answer["people"].array ?? []
        ScrollView {
            if people.isEmpty {
                ContentUnavailableView {
                    Label("No People Yet", systemImage: "person.2")
                } description: {
                    Text("LightCraft finds faces in your photos on this device. Turn it on to group them by person.")
                } actions: {
                    Button("Find People") {
                        Task {
                            await model.run("vision.setFaces", ["on": true])
                            await load()
                        }
                    }
                    .buttonStyle(.borderedProminent)
                }
                .padding(.top, 60)
            } else {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 100), spacing: 16)], spacing: 20) {
                    ForEach(Array(people.enumerated()), id: \.offset) { _, person in
                        Button {
                            if let n = person["name"].string {
                                model.path.append(.person(n))
                            } else {
                                name = ""
                                naming = person
                            }
                        } label: {
                            VStack(spacing: 6) {
                                FaceDisc(photo: person["cover"]["photo"].int.map { UInt64($0) })
                                    .frame(width: 96, height: 96)
                                Text(person["name"].string ?? String(localized: "Add Name"))
                                    .font(.subheadline.weight(.semibold))
                                    .foregroundStyle(person["name"].string == nil ? Color.accentColor : .primary)
                                Text("\(person["photos"].int ?? 0) photos").font(.caption).foregroundStyle(.secondary)
                            }
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(16)
            }
        }
        .navigationTitle("People")
        .task(id: model.state.catalogRevision) { await load() }
        .alert("Who Is This?", isPresented: Binding(get: { naming != nil }, set: { if !$0 { naming = nil } })) {
            TextField("Name", text: $name)
            Button("Done") {
                if let p = naming, !name.isEmpty {
                    Task {
                        await model.run("people.name", ["cluster": p["id"], "name": .string(name)])
                        await load()
                    }
                }
                naming = nil
            }
            Button("Cancel", role: .cancel) { naming = nil }
        }
    }

    private func load() async {
        guard let engine = model.engine else { return }
        answer = (try? await engine.execute("people.list", ["all": true])) ?? .null
    }
}

/// A person's photos and naming.
struct PersonView: View {
    @Environment(AppModel.self) private var model
    let name: String
    @State private var info: JSONValue = .null

    var body: some View {
        List {
            Section {
                VStack(spacing: 8) {
                    Text(name).font(.title.bold())
                    Text("\(info["total"].int ?? 0) photos").foregroundStyle(.secondary)
                    Button("Show Photos") {
                        Task {
                            await model.run("library.filter", ["person": .string(name)])
                            model.prefs.gridSpan = .all
                            model.path = []
                        }
                    }
                    .buttonStyle(.borderedProminent)
                }
                .frame(maxWidth: .infinity)
            }
        }
        .navigationTitle(name)
        .navigationBarTitleDisplayMode(.inline)
        .task {
            guard let engine = model.engine else { return }
            info = (try? await engine.execute("faces.person", ["name": .string(name)])) ?? .null
        }
    }
}

/// A round crop of a photo (a face's photo until face crops render).
private struct FaceDisc: View {
    @Environment(AppModel.self) private var model
    let photo: UInt64?

    var body: some View {
        Circle()
            .fill(Color(uiColor: .tertiarySystemFill))
            .overlay {
                if let photo, let p = model.images.thumb(photo) {
                    Image(decorative: p.image, scale: 1).resizable().scaledToFill()
                } else {
                    Image(systemName: "person.fill").font(.largeTitle).foregroundStyle(.secondary)
                }
            }
            .clipShape(Circle())
            .task(id: photo) { if let photo { model.wantThumb(photo, edge: 256) } }
            .onDisappear { if let photo { model.thumbDisappeared(photo) } }
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
                        MapMarker(front: m["front"].int.map { UInt64($0) }, count: Int(m["count"].int ?? 0))
                            .onTapGesture { show(m) }
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
            if located == 0 && !markers.isEmpty == false {
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

/// A cluster on the map: its front photo framed in white, the count in a badge.
private struct MapMarker: View {
    @Environment(AppModel.self) private var model
    let front: UInt64?
    let count: Int

    var body: some View {
        RoundedRectangle(cornerRadius: 8)
            .fill(.white)
            .frame(width: 52, height: 52)
            .overlay {
                if let front, let p = model.images.thumb(front) {
                    Image(decorative: p.image, scale: 1).resizable().scaledToFill().frame(width: 46, height: 46).clipShape(.rect(cornerRadius: 6))
                }
            }
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
            .task(id: front) { if let front { model.wantThumb(front, edge: 128) } }
            .onDisappear { if let front { model.thumbDisappeared(front) } }
            .accessibilityLabel("\(count) photos")
    }
}
#endif
