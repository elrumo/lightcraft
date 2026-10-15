import LightCraftCore
import SwiftUI

/// The Edit tool's Lens Blur group (`lensBlur.*`, the `lensBlur` section's sliders): Apply (the
/// photo's own depth, or the depth model's estimate, with the model's download offered first),
/// then Blur Amount, the focal range, focus on a point (tap the photo) or the subject, Visualize
/// Depth, the bokeh, Boost, and the Refine brushes painted on the photo (`LensBrushCanvas`).
struct LensBlurGroup: View {
    @Environment(AppModel.self) private var model
    /// `lensBlur.info`.
    @State private var info: JSONValue = .null
    /// The depth model in `models.list`, while it is offered or downloading.
    @State private var offer: JSONValue = .null
    @State private var estimating = false

    static let modelID = "depth-anything-v2-small"

    var body: some View {
        let editor = model.editor
        VStack(alignment: .leading, spacing: 8) {
            if info["enabled"].bool == true {
                Toggle("Apply", isOn: Binding(get: { true }, set: { on in if !on { model.perform("lensBlur.apply", ["on": false]) } }))
                Text(depthNote).font(.footnote).foregroundStyle(.secondary)
                Sliders(section: "lensBlur") { $0.id == "lensBlur.amount" }
                Text("Focal Range").font(.subheadline.weight(.semibold))
                FocalRangeBar(near: info["near"].double ?? 0, far: info["far"].double ?? 0)
                HStack {
                    Button("Focus on a Point", systemImage: "scope") {
                        editor.lensBrush = nil
                        editor.picking = .lensFocus
                        model.show(String(localized: "Tap what should be in focus"))
                    }
                    Button("Subject", systemImage: "person.crop.rectangle") { model.perform("lensBlur.focus", ["subject": true]) }
                }
                .buttonStyle(.bordered)
                Toggle("Visualize Depth", isOn: Binding(get: { editor.visualizeDepth }, set: { editor.visualizeDepth = $0 }))
                Picker("Bokeh", selection: Binding(get: { info["bokeh"].string ?? "circle" }, set: { model.perform("lensBlur.bokeh", ["shape": .string($0)]) })) {
                    ForEach(info["bokehs"].array ?? [], id: \.self) { b in
                        Text(LocalizedStringKey(b["label"].string ?? "")).tag(b["id"].string ?? "")
                    }
                }
                Sliders(section: "lensBlur") { $0.id == "lensBlur.boost" }
                refine
            } else {
                Text("Blur what is nearer or farther than the subject, as a wide aperture would, using the photo's depth.")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                Button {
                    Task { await apply() }
                } label: {
                    if estimating { ProgressView() } else { Text("Apply") }
                }
                .buttonStyle(.borderedProminent)
                .disabled(estimating)
                if !offer.isNull { modelOffer }
            }
        }
        .padding(.vertical, 8)
        // (the brush and the focus picker belong to this group: leaving it puts them down)
        .onDisappear {
            model.editor.lensBrush = nil
            if model.editor.picking == .lensFocus { model.editor.picking = nil }
        }
        .task(id: model.state.catalogRevision) { await load() }
        .task(id: offer["download"]["running"].bool == true) { await watchDownload() }
    }

    private var depthNote: LocalizedStringKey {
        switch info["depth"].string {
        case "estimated": "Depth estimated by AI (Depth Anything V2 Small)."
        case "photo": "Depth from the photo."
        default: "No depth yet: turn Lens Blur off and on to estimate it."
        }
    }

    /// Refine: Focus / Blur / Erase painted on the photo, the brush and Clear.
    @ViewBuilder private var refine: some View {
        @Bindable var editor = model.editor
        Text("Refine").font(.subheadline.weight(.semibold))
        Picker("Brush", selection: Binding(get: { editor.lensBrush ?? "" }, set: { editor.lensBrush = $0.isEmpty ? nil : $0; editor.picking = nil })) {
            Text("Off").tag("")
            Text("Focus").tag("focus")
            Text("Blur").tag("blur")
            Text("Erase").tag("erase")
        }
        .pickerStyle(.segmented)
        if editor.lensBrush != nil {
            Text("Paint on the photo: Focus keeps what you paint sharp, Blur blurs it, Erase takes either back.")
                .font(.footnote)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Toggle("Auto Mask", isOn: $editor.brushAutoMask)
            LabeledContent("Size") { Slider(value: $editor.brushSize, in: 0.0025...0.25) }
            LabeledContent("Feather") { Slider(value: $editor.brushFeather, in: 0...100) }
            LabeledContent("Flow") { Slider(value: $editor.brushFlow, in: 1...100) }
        }
        if (info["focusStrokes"].int ?? 0) + (info["blurStrokes"].int ?? 0) > 0 {
            Button("Clear Brushing", role: .destructive) { model.perform("lensBlur.clearBrush") }
                .buttonStyle(.bordered)
        }
    }

    /// The depth model's download offer (size, licence and credit first), or its progress.
    @ViewBuilder private var modelOffer: some View {
        Group {
            if offer["available"].bool == false {
                Text("This photo has no depth of its own, and depth estimation isn't available in this build.")
            } else if offer["enabled"].bool == false {
                Text("Depth estimation is turned off in Settings → AI Models.")
            } else if offer["download"]["running"].bool == true {
                let d = offer["download"], total = max(d["total"].double ?? 0, 1)
                HStack {
                    ProgressView(value: (d["done"].double ?? 0) / total) { Text("Downloading the depth model…") }
                    Button("Cancel Download") { model.perform("models.cancel", ["id": .string(Self.modelID)]) }
                }
            } else {
                Text("This photo has no depth of its own: Lens Blur estimates it with Depth Anything V2 Small, a \(Int((offer["downloadBytes"].double ?? 0) / 1e6)) MB download (\(offer["licence"].string ?? "") licence). \(offer["credit"].string ?? "").")
                if let e = offer["download"]["error"].string { Text(e).foregroundStyle(.red) }
                Button("Download") { model.perform("models.download", ["id": .string(Self.modelID), "acknowledged": true]) }
                    .buttonStyle(.bordered)
            }
        }
        .font(.footnote)
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)
    }

    private func load() async {
        guard let engine = model.engine else { return }
        info = (try? await engine.execute("lensBlur.info")) ?? .null
    }

    /// Turn it on: the photo's own depth at once; else the model estimates it (a second or two,
    /// "Estimating depth…" meanwhile); without the model, its download is offered instead.
    private func apply() async {
        guard let engine = model.engine else { return }
        if info["depth"].isNull, info["modelReady"].bool != true {
            // (the photo may have depth of its own: the engine knows once it has read it)
            if (try? await engine.execute("lensBlur.apply")) != nil {
                await load()
                return
            }
            await refreshOffer()
            return
        }
        estimating = true
        if info["depth"].isNull { model.show(String(localized: "Estimating depth…")) }
        _ = await model.run("lensBlur.apply")
        estimating = false
        await load()
    }

    private func refreshOffer() async {
        guard let engine = model.engine, let list = (try? await engine.execute("models.list"))?["models"].array else { return }
        offer = list.first { $0["id"].string == Self.modelID } ?? .null
    }

    /// While the model downloads, its progress; once it's here, Apply runs.
    private func watchDownload() async {
        guard !offer.isNull else { return }
        while !Task.isCancelled {
            try? await Task.sleep(for: .milliseconds(400))
            await refreshOffer()
            if offer["installed"].bool == true {
                offer = .null
                await load()
                await apply()
                return
            }
            if offer["download"]["running"].bool != true { return }
        }
    }
}

/// The focal range on a near-to-far bar in Visualize Depth's colours: drag its ends to widen or
/// narrow it, its middle to move it (`develop.set` of `lensBlur.focalDistance` and
/// `lensBlur.focalRange`, one undo step per drag).
struct FocalRangeBar: View {
    @Environment(AppModel.self) private var model
    let near: Double
    let far: Double
    @State private var live: (near: Double, far: Double)?
    @State private var grip: Grip?

    private enum Grip {
        case near, far, range(offset: Double)
    }

    private static let nearColor = Color(red: 240 / 255, green: 160 / 255, blue: 48 / 255)
    private static let farColor = Color(red: 48 / 255, green: 80 / 255, blue: 192 / 255)

    var body: some View {
        let (n, f) = live ?? (near, far)
        GeometryReader { geo in
            let w = max(geo.size.width, 1)
            let x = { (v: Double) in CGFloat(v / 100) * w }
            ZStack(alignment: .leading) {
                LinearGradient(colors: [Self.nearColor, Self.farColor], startPoint: .leading, endPoint: .trailing)
                Color.black.opacity(0.55).frame(width: x(n))
                Color.black.opacity(0.55).frame(width: w - x(f)).offset(x: x(f))
                RoundedRectangle(cornerRadius: 4)
                    .strokeBorder(.white, lineWidth: 2)
                    .frame(width: max(x(f) - x(n), 6))
                    .offset(x: x(n))
            }
            .clipShape(RoundedRectangle(cornerRadius: 5))
            .contentShape(Rectangle())
            .gesture(drag(width: w))
        }
        .frame(height: 28)
        .accessibilityElement()
        .accessibilityLabel(Text("Focal Range"))
        .accessibilityValue(Text("\(Int(n.rounded()))–\(Int(f.rounded()))"))
    }

    private func drag(width: CGFloat) -> some Gesture {
        DragGesture(minimumDistance: 2)
            .onChanged { g in
                let v = { (x: CGFloat) in min(max(Double(x / width) * 100, 0), 100) }
                let (n, f) = live ?? (near, far)
                if grip == nil {
                    let start = v(g.startLocation.x)
                    let reach = 8.0
                    grip = abs(start - n) <= reach && abs(start - n) < abs(start - f) ? .near : abs(start - f) <= reach ? .far : .range(offset: start - (n + f) / 2)
                    model.editor.setMany(values(n, f), label: "Focal Range", began: true, ended: false, model)
                }
                let at = v(g.location.x)
                var next = (n, f)
                switch grip {
                case .near: next = (min(at, f), f)
                case .far: next = (n, max(at, n))
                case .range(let offset):
                    let half = (f - n) / 2
                    let c = min(max(at - offset, half), 100 - half)
                    next = (c - half, c + half)
                case nil: break
                }
                live = next
                model.editor.setMany(values(next.0, next.1), label: "Focal Range", began: false, ended: false, model)
            }
            .onEnded { _ in
                let (n, f) = live ?? (near, far)
                model.editor.setMany(values(n, f), label: "Focal Range", began: false, ended: true, model)
                grip = nil
                live = nil
            }
    }

    private func values(_ n: Double, _ f: Double) -> [String: Double] {
        ["lensBlur.focalDistance": (n + f) / 2, "lensBlur.focalRange": max(f - n, 0)]
    }
}

/// Lens Blur's Refine brush on the photo: the stroke drawn as it is painted, then one
/// `lensBlur.brushStroke` (Focus, Blur or Erase, with the brush's size, feather and flow).
struct LensBrushCanvas: View {
    @Environment(AppModel.self) private var model
    let photo: UInt64
    /// Where the shown photo is drawn.
    let rect: CGRect
    @State private var frame: PhotoFrame?
    @State private var trail: [CGPoint] = []

    var body: some View {
        let mode = model.editor.lensBrush ?? "focus"
        Canvas { ctx, _ in
            guard trail.count > 0 else { return }
            var p = Path()
            p.addLines(trail)
            let color: Color = mode == "focus" ? .green : mode == "blur" ? .blue : .red
            ctx.stroke(p, with: .color(color.opacity(0.4)), style: StrokeStyle(lineWidth: brushRadius * 2, lineCap: .round, lineJoin: .round))
        }
        .allowsHitTesting(false)
        .overlay { Color.clear.contentShape(Rectangle()).gesture(paint(mode)) }
        .task(id: photo) {
            frame = try? await model.engine?.execute("view.frame", ["id": .int(Int64(clamping: photo))], as: PhotoFrame.self)
        }
    }

    private var brushRadius: CGFloat {
        CGFloat(model.editor.brushSize) * max(rect.width, rect.height)
    }

    private func norm(_ p: CGPoint) -> (x: Double, y: Double)? {
        guard let frame, rect.width > 0, rect.height > 0 else { return nil }
        return frame.norm(u: (p.x - rect.minX) / rect.width, v: (p.y - rect.minY) / rect.height)
    }

    private func paint(_ mode: String) -> some Gesture {
        DragGesture(minimumDistance: 0)
            .onChanged { g in
                if trail.last.map({ hypot($0.x - g.location.x, $0.y - g.location.y) > 2 }) ?? true {
                    trail.append(g.location)
                }
            }
            .onEnded { _ in
                let pts: [JSONValue] = trail.compactMap { p in norm(p).map { [.double($0.x), .double($0.y)] } }
                trail = []
                guard !pts.isEmpty else { return }
                let e = model.editor
                model.perform("lensBlur.brushStroke", [
                    "mode": .string(mode), "points": .array(pts), "size": .double(e.brushSize), "feather": .double(e.brushFeather),
                    "flow": .double(e.brushFlow), "autoMask": .bool(e.brushAutoMask),
                ])
            }
    }
}
