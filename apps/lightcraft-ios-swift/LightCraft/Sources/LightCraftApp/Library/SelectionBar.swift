#if os(iOS)
import LightCraftCore
import SwiftUI

/// The bar under the grid while photos are chosen: rate, flag, label, add to an album, export, more,
/// delete. Adding to an album from its + shows Add instead.
struct SelectionBar: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let ids = Array(model.library.selected)
        let none = ids.isEmpty
        Group {
            if let album = model.addingTo {
                HStack {
                    Text("\(ids.count) Selected").font(.headline)
                    Spacer()
                    Button("Add") { Task { await add(ids, to: album) } }
                        .buttonStyle(.glassProminent)
                        .disabled(none)
                }
                .padding(.horizontal, 20)
                .frame(height: 56)
            } else {
                HStack(spacing: 0) {
                    Menu {
                        RatingItems(ids: ids)
                    } label: { barIcon("star", "Rating") }
                    Menu {
                        FlagItems(ids: ids)
                    } label: { barIcon("flag", "Flag") }
                    Menu {
                        LabelItems(ids: ids)
                    } label: { barIcon("circle.lefthalf.filled", "Color Label") }
                    Menu {
                        AlbumItems(ids: ids)
                    } label: { barIcon("rectangle.stack.badge.plus", "Add to Album") }
                    Button {
                        model.sheet = .export(ids)
                    } label: { barIcon("square.and.arrow.up", "Export") }
                    Menu {
                        SelectionMoreItems(ids: ids)
                    } label: { barIcon("ellipsis.circle", "More") }
                    DeleteButton(ids: ids) { barIcon("trash", "Delete") }
                }
                .disabled(none)
                .frame(height: 56)
            }
        }
        .glassEffect(.regular, in: .capsule)
        .frame(maxWidth: 480)
        .padding(.horizontal, 14)
        .padding(.bottom, 4)
    }

    private func barIcon(_ symbol: String, _ label: LocalizedStringKey) -> some View {
        Image(systemName: symbol)
            .font(.title3)
            .frame(maxWidth: .infinity, minHeight: 44)
            .contentShape(Rectangle())
            .accessibilityLabel(label)
    }

    private func add(_ ids: [UInt64], to album: UInt64) async {
        if let r = await model.run("album.addPhotos", ["id": .int(Int64(clamping: album)), "ids": AppModel.ids(ids)]) {
            let added = r["added"].int ?? Int64(ids.count)
            model.show(String(localized: "Added \(added) photos"))
        }
        model.selectMode = false
        model.addingTo = nil
        model.perform("library.source", ["kind": "album", "id": .int(Int64(clamping: album))])
    }
}

/// None and 1–5 stars, for `ids`.
struct RatingItems: View {
    @Environment(AppModel.self) private var model
    let ids: [UInt64]

    var body: some View {
        Section("Rating") {
            ForEach(0...5, id: \.self) { r in
                Button {
                    model.perform("photo.rate", ["ids": AppModel.ids(ids), "rating": .int(Int64(r))])
                } label: {
                    if r == 0 {
                        Label("No Rating", systemImage: "star.slash")
                    } else {
                        Text(String(repeating: "★", count: r))
                    }
                }
            }
        }
    }
}

/// Pick, Reject, Unflagged.
struct FlagItems: View {
    @Environment(AppModel.self) private var model
    let ids: [UInt64]

    var body: some View {
        Section("Flag") {
            Button("Pick", systemImage: "flag.fill") { model.perform("photo.flag", ["ids": AppModel.ids(ids), "flag": "pick"]) }
            Button("Reject", systemImage: "flag.slash.fill") { model.perform("photo.flag", ["ids": AppModel.ids(ids), "flag": "reject"]) }
            Button("Unflagged", systemImage: "flag") { model.perform("photo.flag", ["ids": AppModel.ids(ids), "flag": "none"]) }
        }
    }
}

/// The five colour labels (by their names in this library) and none.
struct LabelItems: View {
    @Environment(AppModel.self) private var model
    let ids: [UInt64]

    var body: some View {
        Section("Color Label") {
            ForEach(LabelColor.all, id: \.self) { label in
                Button {
                    setLabel(label)
                } label: {
                    Label {
                        Text(model.library.labelName(label))
                    } icon: {
                        Image(systemName: "circle.fill").foregroundStyle(LabelColor.color(label))
                    }
                }
            }
            Button("No Label", systemImage: "circle.slash") { setLabel("none") }
        }
    }

    private func setLabel(_ label: String) {
        Task {
            guard await model.run("photo.label", ["ids": AppModel.ids(ids), "label": .string(label)]) != nil else { return }
            model.show(label == "none" ? String(localized: "Color label cleared") : String(localized: "\(model.library.labelName(label)) Label"))
        }
    }
}

/// Plain albums, A–Z, to add `ids` to.
struct AlbumItems: View {
    @Environment(AppModel.self) private var model
    let ids: [UInt64]

    var body: some View {
        Section("Add to Album") {
            ForEach(model.albums.plain) { album in
                Button(album.name, systemImage: "rectangle.stack") {
                    Task {
                        if let r = await model.run("album.addPhotos", ["id": .int(Int64(clamping: album.id)), "ids": AppModel.ids(ids)]) {
                            model.show(String(localized: "Added \(r["added"].int ?? Int64(ids.count)) to \(album.name)"))
                        }
                    }
                }
            }
            Button("New Album…", systemImage: "plus") {
                model.sheet = .newAlbum(parent: nil, folder: false, addSelected: true)
            }
        }
    }
}

/// Copy and paste edit settings, and editing the first chosen photo.
struct SelectionMoreItems: View {
    @Environment(AppModel.self) private var model
    let ids: [UInt64]

    var body: some View {
        Button("Copy Edit Settings", systemImage: "doc.on.doc") {
            Task {
                guard let first = ids.first else { return }
                await model.run("library.select", ["ids": [.int(Int64(clamping: first))]])
                if await model.run("develop.copy") != nil { model.show(String(localized: "Edit settings copied")) }
            }
        }
        .disabled(ids.count != 1)
        Button("Paste Edit Settings", systemImage: "doc.on.clipboard") {
            model.perform("develop.paste", ["ids": AppModel.ids(ids)])
        }
        .disabled(model.library.state?.clipboard != true)
        Button("Paste Selected Settings…", systemImage: "checklist") { model.sheet = .pasteSettings(ids) }
            .disabled(model.library.state?.clipboard != true)
        Button("Edit", systemImage: "slider.horizontal.3") {
            guard let first = ids.first else { return }
            model.selectMode = false
            model.open(first)
        }
        PhotoManageItems(ids: ids)
    }
}

/// Rename, capture time and metadata presets for `ids` (the select bar's and a photo's "…").
struct PhotoManageItems: View {
    @Environment(AppModel.self) private var model
    let ids: [UInt64]

    var body: some View {
        Section {
            Button("Rename…", systemImage: "character.cursor.ibeam") { model.sheet = .renamePhotos(ids) }
            Button("Edit Capture Time…", systemImage: "calendar.badge.clock") { model.sheet = .captureTime(ids) }
            Button("Metadata Presets…", systemImage: "text.badge.checkmark") { model.sheet = .metadataPresets(ids) }
            Button("Contact Sheet…", systemImage: "doc.richtext") { model.sheet = .contactSheet(ids) }
        }
        .disabled(ids.isEmpty)
        if ids.count > 1 {
            Section {
                Button("Compare", systemImage: "rectangle.split.2x1") {
                    model.selectMode = false
                    model.path.append(.compare(ids))
                }
                Button("Survey", systemImage: "square.grid.2x2") {
                    model.selectMode = false
                    model.path.append(.survey(ids))
                }
            }
            Menu("Photo Merge", systemImage: "square.on.square") {
                ForEach([("merge.hdr", "HDR…"), ("merge.panorama", "Panorama…"), ("merge.hdrPanorama", "HDR Panorama…")], id: \.0) { command, label in
                    Button(LocalizedStringKey(label)) { model.sheet = .merge(command, ids) }
                }
                Divider()
                ForEach([("merge.hdr", "HDR with Last Settings"), ("merge.panorama", "Panorama with Last Settings"), ("merge.hdrPanorama", "HDR Panorama with Last Settings")], id: \.0) { command, label in
                    Button(LocalizedStringKey(label)) { Task { await model.request("host.merge.last", ["command": .string(command), "ids": AppModel.ids(ids)]) } }
                }
            }
        }
    }
}

/// Delete: in a plain album, a choice between taking the photos out of it and deleting them;
/// deleting asks first.
struct DeleteButton<LabelView: View>: View {
    @Environment(AppModel.self) private var model
    let ids: [UInt64]
    @ViewBuilder let label: () -> LabelView

    var body: some View {
        if let album = model.plainAlbum {
            Menu {
                Button("Remove from Album", systemImage: "rectangle.stack.badge.minus") {
                    Task {
                        await model.run("album.removePhotos", ["id": .int(Int64(clamping: album)), "ids": AppModel.ids(ids)])
                        await model.run("library.select", ["ids": []])
                    }
                }
                Button("Delete", systemImage: "trash", role: .destructive) { model.confirmDelete = ids }
            } label: { label() }
        } else {
            Button(role: .destructive) { model.confirmDelete = ids } label: { label() }
                .haptic(.warning, trigger: model.confirmDelete)
        }
    }
}
#endif
