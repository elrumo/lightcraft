#if os(iOS)
import LightCraftCore
import SwiftUI

/// Red Eye on the photo: each correction's ellipse (the chosen one bright); drag a box around an
/// eye to correct it (`redeye.add`: the pupil inside is found by the engine), tap one to choose it.
struct RedEyeCanvas: View {
    @Environment(AppModel.self) private var model
    let photo: UInt64
    /// Where the shown photo is drawn.
    let rect: CGRect
    @State private var frame: PhotoFrame?
    @State private var box: (CGPoint, CGPoint)?

    var body: some View {
        let eyes = model.editor.setting("red_eye").array ?? []
        Canvas { ctx, _ in
            for (i, e) in eyes.enumerated() {
                guard let r = ellipse(e) else { continue }
                let chosen = i == model.editor.eye
                ctx.stroke(Path(ellipseIn: r), with: .color(.white.opacity(chosen ? 1 : 0.6)), lineWidth: chosen ? 1.5 : 1)
                var cross = Path()
                cross.move(to: CGPoint(x: r.midX - 4, y: r.midY))
                cross.addLine(to: CGPoint(x: r.midX + 4, y: r.midY))
                cross.move(to: CGPoint(x: r.midX, y: r.midY - 4))
                cross.addLine(to: CGPoint(x: r.midX, y: r.midY + 4))
                ctx.stroke(cross, with: .color(.white.opacity(chosen ? 1 : 0.6)), lineWidth: 1)
            }
            if let (a, b) = box {
                ctx.stroke(Path(ellipseIn: CGRect(x: min(a.x, b.x), y: min(a.y, b.y), width: abs(b.x - a.x), height: abs(b.y - a.y))), with: .color(.white), lineWidth: 1)
            }
        }
        .allowsHitTesting(false)
        .overlay {
            Color.clear.contentShape(Rectangle())
                .highPriorityGesture(drag)
                .simultaneousGesture(SpatialTapGesture().onEnded { tap in
                    if let i = eyes.indices.first(where: { ellipse(eyes[$0])?.insetBy(dx: -12, dy: -12).contains(tap.location) == true }) {
                        model.editor.eye = i
                    }
                })
        }
        .task(id: model.state.catalogRevision) {
            guard let engine = model.engine else { return }
            frame = try? await engine.execute("view.frame", ["id": .int(Int64(clamping: photo))], as: PhotoFrame.self)
        }
        .accessibilityElement()
        .accessibilityLabel("Red Eye")
        .accessibilityHint("Drag a box around an eye to correct it")
    }

    /// Screen points per long-edge unit of the photo (red eye radii are in those).
    private var perLong: CGFloat? {
        guard let o = screen(0, 0), let x = screen(1, 0), let y = screen(0, 1) else { return nil }
        return max(hypot(x.x - o.x, x.y - o.y), hypot(y.x - o.x, y.y - o.y))
    }

    private func screen(_ x: Double, _ y: Double) -> CGPoint? {
        guard let frame else { return nil }
        let s = frame.shown(x: x, y: y)
        return CGPoint(x: rect.minX + s.u * rect.width, y: rect.minY + s.v * rect.height)
    }

    private func norm(_ p: CGPoint) -> (x: Double, y: Double)? {
        guard let frame, rect.width > 0, rect.height > 0 else { return nil }
        return frame.norm(u: (p.x - rect.minX) / rect.width, v: (p.y - rect.minY) / rect.height)
    }

    /// A correction's ellipse on screen.
    private func ellipse(_ e: JSONValue) -> CGRect? {
        let c = e["center"]
        guard let x = c["x"].double ?? c.array?.first?.double, let y = c["y"].double ?? c.array?.last?.double,
              let p = screen(x, y), let per = perLong, let rx = e["rx"].double, let ry = e["ry"].double else { return nil }
        let (w, h) = (CGFloat(rx) * per, CGFloat(ry) * per)
        return CGRect(x: p.x - w, y: p.y - h, width: w * 2, height: h * 2)
    }

    private var drag: some Gesture {
        DragGesture(minimumDistance: 4)
            .onChanged { g in box = (g.startLocation, g.location) }
            .onEnded { g in
                box = nil
                let (a, b) = (g.startLocation, g.location)
                guard let per = perLong, per > 0, let c = norm(CGPoint(x: (a.x + b.x) / 2, y: (a.y + b.y) / 2)) else { return }
                let (rx, ry) = (abs(b.x - a.x) / 2 / per, abs(b.y - a.y) / 2 / per)
                guard rx > 0.002, ry > 0.002 else { return }
                Task {
                    let r = await model.run("redeye.add", ["center": [.double(c.x), .double(c.y)], "rx": .double(rx), "ry": .double(ry), "pet": .bool(model.editor.eyePet)])
                    if let i = r?["index"].int { model.editor.eye = Int(i) }
                }
            }
    }
}

/// Red Eye's controls: red or pet eyes, the chosen correction's pupil size and darkening (a pet's
/// catchlight), Delete, and ✕ Red Eye ✓.
struct RedEyeTools: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var editor = model.editor
        let eyes = editor.setting("red_eye").array ?? []
        VStack(spacing: 10) {
            Picker("Eyes", selection: $editor.eyePet) {
                Text("Red Eye").tag(false)
                Text("Pet Eye").tag(true)
            }
            .pickerStyle(.segmented)
            .padding(.horizontal, 16)
            if eyes.indices.contains(editor.eye) {
                VStack(spacing: 0) {
                    Sliders(section: "redEye") { $0.id.hasPrefix("redEye.\(editor.eye).") }
                }
                .padding(.horizontal, 16)
                HStack {
                    if eyes[editor.eye]["pet"].bool == true {
                        Toggle("Catchlight", isOn: Binding(get: { !eyes[editor.eye]["catchlight"].isNull }, set: { on in
                            model.perform("redeye.catchlight", ["index": .int(Int64(editor.eye)), "on": .bool(on)])
                        }))
                    }
                    Spacer()
                    Button("Delete", systemImage: "trash", role: .destructive) {
                        model.perform("redeye.delete", ["index": .int(Int64(editor.eye))])
                        editor.eye = max(editor.eye - 1, 0)
                    }
                }
                .padding(.horizontal, 16)
            } else {
                Text("Drag a box around an eye.").font(.footnote).foregroundStyle(.secondary)
            }
        }
        .padding(.vertical, 8)
        .onAppear { editor.want(["redEye"], model) }
    }
}
#endif
