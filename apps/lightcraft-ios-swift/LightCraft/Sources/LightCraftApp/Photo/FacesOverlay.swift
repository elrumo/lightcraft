#if os(iOS)
import LightCraftCore
import SwiftUI

/// The faces on the photo (Faces in the photo's "…" menu): each framed, with its name under it, or
/// who it looks like with a "?", or "Name?"; a tap on the name names it. With Tag a Face, a tap on
/// the photo marks a face there (`photo.addRegion {shown}`).
struct FacesOverlay: View {
    @Environment(AppModel.self) private var model
    let photo: UInt64
    /// Where the shown photo is drawn.
    let rect: CGRect
    @State private var frame: PhotoFrame?
    @State private var faces: [Face] = []
    /// What recognition suggests for the unnamed faces (`faces.suggest`): index → (best, candidates).
    @State private var hints: [Int: (String?, [String])] = [:]
    @State private var naming: FaceNaming?

    var body: some View {
        ZStack {
            Color.clear
                .contentShape(Rectangle())
                .allowsHitTesting(model.editor.tagFace)
                .onTapGesture { tag(at: $0) }
            ForEach(faces) { face in
                if let box = box(face) {
                    RoundedRectangle(cornerRadius: 8)
                        .stroke(.white.opacity(0.9), lineWidth: 1.5)
                        .shadow(color: .black.opacity(0.5), radius: 1)
                        .frame(width: box.width, height: box.height)
                        .position(x: box.midX, y: box.midY)
                        .allowsHitTesting(false)
                    Button {
                        let h = hints[face.index]
                        naming = FaceNaming(photo: photo, index: face.index, rect: nil, start: face.name ?? h?.0 ?? "", candidates: h?.1 ?? [], crop: crop(box))
                    } label: {
                        Text(label(face))
                            .font(.footnote.weight(.semibold))
                            .foregroundStyle(face.name == nil ? Color.white.opacity(0.8) : .white)
                            .padding(.horizontal, 9).padding(.vertical, 4)
                            .background(.black.opacity(0.62), in: .capsule)
                    }
                    .buttonStyle(.plain)
                    .position(x: box.midX, y: box.maxY + 18)
                    .accessibilityLabel(face.name ?? String(localized: "Unnamed face"))
                    .accessibilityHint("Name this face")
                }
            }
            if model.editor.tagFace {
                Text("Tap a face to mark it")
                    .font(.subheadline.weight(.semibold)).foregroundStyle(.white)
                    .padding(.horizontal, 14).padding(.vertical, 7)
                    .background(.black.opacity(0.66), in: .capsule)
                    .position(x: rect.midX, y: max(rect.minY, 0) + 26)
                    .allowsHitTesting(false)
            }
        }
        .sheet(item: $naming) { FaceNameSheet(target: $0) }
        .task(id: Key(photo: photo, revision: model.state.catalogRevision)) { await load() }
    }

    private nonisolated struct Key: Hashable {
        let photo: UInt64
        let revision: UInt64
    }

    private func load() async {
        guard let engine = model.engine else { return }
        let id: JSONValue = .int(Int64(clamping: photo))
        frame = try? await engine.execute("view.frame", ["id": id], as: PhotoFrame.self)
        faces = (try? await engine.execute("photo.faces", ["id": id], as: [Face].self)) ?? []
        // recognition's guesses for the unnamed ones (nothing new is computed: budget 0)
        var next: [Int: (String?, [String])] = [:]
        if let answer = try? await engine.execute("faces.suggest", ["ids": [id], "budgetMs": 0]) {
            for f in answer["photos"][0]["faces"].array ?? [] {
                guard let i = f["index"].int else { continue }
                next[Int(i)] = (f["suggestion"]["name"].string, (f["candidates"].array ?? []).compactMap { $0["name"].string })
            }
        }
        hints = next
    }

    private func label(_ face: Face) -> String {
        if let name = face.name { return name }
        if let guess = hints[face.index]?.0 { return "\(guess)?" }
        return String(localized: "Name?")
    }

    /// A face's box on screen (around its corners: the photo may be straightened).
    private func box(_ face: Face) -> CGRect? {
        guard let frame, face.rect.count == 4 else { return nil }
        let r = face.rect
        let corners = [(r[0], r[1]), (r[2], r[1]), (r[2], r[3]), (r[0], r[3])].map { x, y in
            let s = frame.shown(x: x, y: y)
            return CGPoint(x: rect.minX + s.u * rect.width, y: rect.minY + s.v * rect.height)
        }
        let xs = corners.map(\.x), ys = corners.map(\.y)
        guard let x0 = xs.min(), let x1 = xs.max(), let y0 = ys.min(), let y1 = ys.max() else { return nil }
        return CGRect(x: x0, y: y0, width: x1 - x0, height: y1 - y0)
    }

    /// The face in the loupe's picture, for the naming sheet.
    private func crop(_ box: CGRect) -> CGImage? {
        guard let image = model.images.view(for: photo)?.image, rect.width > 0, rect.height > 0 else { return nil }
        let sx = CGFloat(image.width) / rect.width, sy = CGFloat(image.height) / rect.height
        let r = CGRect(x: (box.minX - rect.minX) * sx, y: (box.minY - rect.minY) * sy, width: box.width * sx, height: box.height * sy)
            .insetBy(dx: -box.width * sx * 0.25, dy: -box.height * sy * 0.25)
            .intersection(CGRect(x: 0, y: 0, width: image.width, height: image.height))
        return r.isEmpty ? nil : image.cropping(to: r)
    }

    /// Tag a Face: a box around the tap, in the photo's shown coordinates.
    private func tag(at p: CGPoint) {
        guard model.editor.tagFace, let frame, rect.contains(p) else { return }
        model.editor.tagFace = false
        let half = 0.09 * min(rect.width, rect.height)
        let norm = { (q: CGPoint) in frame.norm(u: (q.x - rect.minX) / rect.width, v: (q.y - rect.minY) / rect.height) }
        let a = norm(CGPoint(x: p.x - half, y: p.y - half)), b = norm(CGPoint(x: p.x + half, y: p.y + half))
        let shown = [min(a.x, b.x), min(a.y, b.y), max(a.x, b.x), max(a.y, b.y)]
        let box = CGRect(x: p.x - half, y: p.y - half, width: half * 2, height: half * 2)
        naming = FaceNaming(photo: photo, index: nil, rect: shown, start: "", candidates: [], crop: crop(box))
    }
}

/// A face box of `photo.faces`.
nonisolated struct Face: Decodable, Identifiable {
    var index: Int
    var rect: [Double]
    var name: String?
    var id: Int { index }
}

/// A face being named: one already on the photo (`index`), or one just marked (`rect`, shown).
struct FaceNaming: Identifiable {
    let id = UUID()
    let photo: UInt64
    let index: Int?
    let rect: [Double]?
    let start: String
    let candidates: [String]
    let crop: CGImage?
}

/// Who Is This?: the face, a name field, who it looks like and the people whose names start with
/// what is typed; Remove Face for a box that isn't a face.
struct FaceNameSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let target: FaceNaming
    @State private var text = ""
    @State private var people: [String] = []

    var body: some View {
        NavigationStack {
            List {
                if let crop = target.crop {
                    Image(decorative: crop, scale: 1)
                        .resizable().scaledToFill()
                        .frame(width: 112, height: 112)
                        .clipShape(Circle())
                        .frame(maxWidth: .infinity)
                        .listRowBackground(Color.clear)
                }
                Section {
                    TextField("Name", text: $text)
                        .textContentType(.name)
                        .submitLabel(.done)
                        .onSubmit(done)
                }
                if !offered.isEmpty {
                    Section("Suggestions") {
                        ForEach(offered, id: \.self) { name in
                            Button(name) {
                                text = name
                                done()
                            }
                        }
                    }
                }
                if let index = target.index {
                    Section {
                        Button("Remove Face", role: .destructive) {
                            model.perform("photo.removeRegion", ["id": .int(Int64(clamping: target.photo)), "index": .int(Int64(index))])
                            dismiss()
                        }
                    }
                }
            }
            .navigationTitle("Who Is This?")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel", systemImage: "xmark") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done", action: done).disabled(text.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            }
        }
        .presentationDetents([.medium, .large])
        .task {
            text = target.start
            guard let engine = model.engine else { return }
            // the people named in the library, most photos first
            let answer = (try? await engine.execute("library.facets", ["field": "person"])) ?? .null
            people = (answer.array ?? []).compactMap { $0["value"].string }
        }
    }

    /// Who it looks like, then the people whose names start with what is typed (8 at most).
    private var offered: [String] {
        let typed = text.trimmingCharacters(in: .whitespaces).lowercased()
        var out = Array(target.candidates.prefix(3))
        for name in people where out.count < 8 {
            if (typed.isEmpty || name.lowercased().hasPrefix(typed)) && !out.contains(where: { $0.caseInsensitiveCompare(name) == .orderedSame }) {
                out.append(name)
            }
        }
        return out
    }

    private func done() {
        let name = text.trimmingCharacters(in: .whitespaces)
        guard !name.isEmpty else { return }
        model.buzz(.light)
        let id: JSONValue = .int(Int64(clamping: target.photo))
        if let index = target.index {
            model.perform("faces.setName", ["id": id, "index": .int(Int64(index)), "name": .string(name)])
        } else if let r = target.rect, r.count == 4 {
            model.perform("photo.addRegion", ["id": id, "rect": ["x0": .double(r[0]), "y0": .double(r[1]), "x1": .double(r[2]), "y1": .double(r[3])], "shown": true, "name": .string(name)])
        }
        dismiss()
    }
}
#endif
