import LightCraftCore
import SwiftUI

/// The Edit tool's Lens Blur group (`lensBlur.*`, the `lensBlur` section's sliders): Apply (the
/// photo's own depth, or the depth model's estimate, with the model's download offered first),
/// then Blur Amount, the focal range, focus on a point (tap the photo) or the subject, Visualize
/// Depth, the bokeh, Boost, and the Refine brushes painted on the photo (`LensBrushCanvas`).
/// A phone lays it out as a column of controls; a Mac, as the inspector's parts (`macContent`).
struct LensBlurGroup: View {
    @Environment(AppModel.self) private var model
    /// `lensBlur.info`.
    @State private var info: JSONValue = .null
    /// The depth model in `models.list`, while it is offered or downloading.
    @State private var offer: JSONValue = .null
    @State private var estimating = false

    static let modelID = "depth-anything-v2-small"

    var body: some View {
        #if os(macOS)
        tracked(VStack(alignment: .leading, spacing: 4) { macContent })
        #else
        tracked(VStack(alignment: .leading, spacing: 8) { phoneContent }.padding(.vertical, 8))
        #endif
    }

    /// What every layout does: read the group's state, follow the model's download, and put the brush and the focus picker
    /// down when the group goes (they belong to it).
    private func tracked(_ content: some View) -> some View {
        content
            .onDisappear {
                model.editor.lensBrush = nil
                if model.editor.picking == .lensFocus { model.editor.picking = nil }
            }
            .task(id: model.state.catalogRevision) { await load() }
            .task(id: offer["download"]["running"].bool == true) { await watchDownload() }
    }

    @ViewBuilder private var phoneContent: some View {
        let editor = model.editor
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

    #if os(macOS)
    /// A Mac's layout: the inspector's parts (`MacInspectorKit`), with the focal range and its two pickers on rows of the
    /// label column, as the sliders above and under them are.
    @ViewBuilder private var macContent: some View {
        let editor = model.editor
        if info["enabled"].bool == true {
            MacCheck(title: "Apply", isOn: Binding(get: { true }, set: { on in if !on { model.perform("lensBlur.apply", ["on": false]) } }))
            MacHint(text: Text(depthNote))
            Sliders(section: "lensBlur") { $0.id == "lensBlur.amount" }
            MacRow(label: "Focal Range") { FocalRangeBar(near: info["near"].double ?? 0, far: info["far"].double ?? 0) }
            MacRow(label: "Focus") {
                HStack(spacing: 4) {
                    MacIconButton(symbol: "scope", title: "Focus on a Point", on: editor.picking == .lensFocus) {
                        if editor.picking == .lensFocus {
                            editor.picking = nil
                        } else {
                            editor.lensBrush = nil
                            editor.picking = .lensFocus
                            model.show(String(localized: "Tap what should be in focus"))
                        }
                    }
                    MacButton(title: "Subject", symbol: "person.crop.rectangle", wide: false) { model.perform("lensBlur.focus", ["subject": true]) }
                }
            }
            MacCheck(title: "Visualize Depth", isOn: Binding(get: { editor.visualizeDepth }, set: { editor.visualizeDepth = $0 }))
            macRefine
            macBokeh
            Sliders(section: "lensBlur") { $0.id == "lensBlur.boost" }
        } else {
            MacHint(text: Text("Blur what is nearer or farther than the subject, as a wide aperture would, using the photo's depth."))
            if estimating {
                HStack(spacing: 6) {
                    ProgressView().controlSize(.small)
                    Text("Estimating depth…").font(.inspector).foregroundStyle(.secondary)
                }
                .frame(minHeight: MacUI.rowHeight)
            } else {
                MacButton(title: "Apply", prominent: true) { Task { await apply() } }
                    .padding(.top, 4)
            }
            if !offer.isNull { macModelOffer }
        }
    }

    /// The bokeh's shape, from the engine's list.
    @ViewBuilder private var macBokeh: some View {
        let shapes = info["bokehs"].array ?? []
        let current = info["bokeh"].string ?? "circle"
        MacRow(label: "Bokeh") {
            MacMenuField(text: Text(LocalizedStringKey(shapes.first { $0["id"].string == current }?["label"].string ?? ""))) {
                ForEach(shapes, id: \.self) { shape in
                    let id = shape["id"].string ?? ""
                    Button {
                        model.perform("lensBlur.bokeh", ["shape": .string(id)])
                    } label: {
                        if id == current {
                            Label(LocalizedStringKey(shape["label"].string ?? ""), systemImage: "checkmark")
                        } else {
                            Text(LocalizedStringKey(shape["label"].string ?? ""))
                        }
                    }
                }
            }
        }
    }

    /// Refine: Focus, Blur or Erase painted on the photo (the active one again puts the brush down), the brush's settings
    /// (Masking's) and Clear.
    @ViewBuilder private var macRefine: some View {
        let editor = model.editor
        MacCaption(text: "Refine")
        MacSegmented(
            options: [("focus", "Focus"), ("blur", "Blur"), ("erase", "Erase")],
            isOn: { editor.lensBrush == $0 },
            choose: { brush in
                editor.lensBrush = editor.lensBrush == brush ? nil : brush
                editor.picking = nil
            }
        )
        if editor.lensBrush != nil {
            MacHint(text: Text("Paint on the photo: Focus keeps what you paint sharp, Blur blurs it, Erase takes either back."))
            MacHint(text: Text("Hold ⌥ to erase."))
            MacBrushSliders()
        }
        if (info["focusStrokes"].int ?? 0) + (info["blurStrokes"].int ?? 0) > 0 {
            MacButton(title: "Clear Brushing", wide: false) { model.perform("lensBlur.clearBrush") }
                .padding(.top, 4)
        }
    }

    /// The depth model's download offer (size, licence and credit first), or its progress.
    @ViewBuilder private var macModelOffer: some View {
        if offer["available"].bool == false {
            MacHint(text: Text("This photo has no depth of its own, and depth estimation isn't available in this build."))
        } else if offer["enabled"].bool == false {
            MacHint(text: Text("Depth estimation is turned off in Settings → AI Models."))
        } else if offer["download"]["running"].bool == true {
            let d = offer["download"], total = max(d["total"].double ?? 0, 1)
            ProgressView(value: (d["done"].double ?? 0) / total) { Text("Downloading the depth model…").font(.inspector) }
                .padding(.top, 4)
            MacButton(title: "Cancel Download", wide: false) { model.perform("models.cancel", ["id": .string(Self.modelID)]) }
        } else {
            MacHint(text: Text("This photo has no depth of its own: Lens Blur estimates it with Depth Anything V2 Small, a \(Int((offer["downloadBytes"].double ?? 0) / 1e6)) MB download (\(offer["licence"].string ?? "") licence). \(offer["credit"].string ?? "")."))
            if let e = offer["download"]["error"].string { MacHint(text: Text(e).foregroundStyle(.red)) }
            MacButton(title: "Download", wide: false) { model.perform("models.download", ["id": .string(Self.modelID), "acknowledged": true]) }
                .padding(.top, 4)
        }
    }
    #endif

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
        #if os(macOS)
        // (the bar leaves room at both ends for the grips, so one at 0 or 100 is whole)
        let inset: CGFloat = 4
        #else
        let inset: CGFloat = 0
        #endif
        GeometryReader { geo in
            let w = max(geo.size.width - 2 * inset, 1)
            let x = { (v: Double) in CGFloat(v / 100) * w }
            ZStack(alignment: .leading) {
                ZStack(alignment: .leading) {
                    LinearGradient(colors: [Self.nearColor, Self.farColor], startPoint: .leading, endPoint: .trailing)
                    Color.black.opacity(0.55).frame(width: x(n))
                    Color.black.opacity(0.55).frame(width: w - x(f)).offset(x: x(f))
                    RoundedRectangle(cornerRadius: 4)
                        .strokeBorder(.white, lineWidth: 2)
                        .frame(width: max(x(f) - x(n), 6))
                        .offset(x: x(n))
                }
                .frame(width: w, height: Self.barHeight)
                .clipShape(RoundedRectangle(cornerRadius: 5))
                .offset(x: inset)
                #if os(macOS)
                // the two ends to take hold of: a pointer wants something to aim at
                ForEach(Array([n, f].enumerated()), id: \.offset) { _, v in
                    RoundedRectangle(cornerRadius: 2)
                        .fill(.white)
                        .frame(width: 6, height: Self.barHeight + 4)
                        .shadow(color: .black.opacity(0.35), radius: 1.5)
                        .offset(x: inset + x(v) - 3)
                        .allowsHitTesting(false)
                }
                #endif
            }
            .frame(maxHeight: .infinity)
            .contentShape(Rectangle())
            .gesture(drag(width: w, inset: inset))
            #if os(macOS)
            // a click on the bar puts the range there; the pointer says it can be dragged
            .simultaneousGesture(SpatialTapGesture().onEnded { tap in centre(at: tap.location.x - inset, width: w) })
            .pointerStyle(.columnResize)
            #endif
        }
        .frame(height: 28)
        .accessibilityElement()
        .accessibilityLabel(Text("Focal Range"))
        .accessibilityValue(Text("\(Int(n.rounded()))–\(Int(f.rounded()))"))
    }

    #if os(macOS)
    private static let barHeight: CGFloat = 20
    #else
    private static let barHeight: CGFloat = 28
    #endif

    #if os(macOS)
    /// A click: the range, as wide as it is, centred where the click was (one undo step).
    private func centre(at x: CGFloat, width: CGFloat) {
        let at = min(max(Double(x / width) * 100, 0), 100)
        let half = (far - near) / 2
        let c = min(max(at, half), 100 - half)
        model.editor.setMany(values(c - half, c + half), label: "Focal Range", began: true, ended: true, model)
    }
    #endif

    /// `inset`: how far the bar starts from the left of the area the gesture is on.
    private func drag(width: CGFloat, inset: CGFloat) -> some Gesture {
        DragGesture(minimumDistance: 2)
            .onChanged { g in
                let v = { (x: CGFloat) in min(max(Double((x - inset) / width) * 100, 0), 100) }
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
    /// Where a hovering pointer or Apple Pencil is: the brush's size drawn there.
    @State private var hover: CGPoint?
    /// ⌥ is held (a Mac): the brush erases, as it does on a computer.
    @State private var optionDown = false
    /// The stroke under way started with ⌥ held.
    @State private var erasing = false

    var body: some View {
        let brush = model.editor.lensBrush ?? "focus"
        // (⌥ turns the brush into Erase while it is held, and a stroke keeps what it started as)
        let mode = trail.isEmpty ? (optionDown ? "erase" : brush) : (erasing ? "erase" : brush)
        let color: Color = mode == "focus" ? .green : mode == "blur" ? .blue : .red
        Canvas { ctx, _ in
            if let hover, trail.isEmpty {
                let r = brushRadius
                let ring = Path(ellipseIn: CGRect(x: hover.x - r, y: hover.y - r, width: r * 2, height: r * 2))
                ctx.stroke(ring, with: .color(.black.opacity(0.5)), lineWidth: 3)
                ctx.stroke(ring, with: .color(color), lineWidth: 1.5)
            }
            guard trail.count > 0 else { return }
            var p = Path()
            p.addLines(trail)
            ctx.stroke(p, with: .color(color.opacity(0.4)), style: StrokeStyle(lineWidth: brushRadius * 2, lineCap: .round, lineJoin: .round))
        }
        .allowsHitTesting(false)
        .overlay { Color.clear.contentShape(Rectangle()).gesture(paint(brush)).onContinuousHover(perform: hovered) }
        #if os(macOS)
        .onModifierKeysChanged(mask: .option, initial: true) { _, new in optionDown = new.contains(.option) }
        #endif
        .task(id: photo) {
            frame = try? await model.engine?.execute("view.frame", ["id": .int(Int64(clamping: photo))], as: PhotoFrame.self)
        }
    }

    private func hovered(_ phase: HoverPhase) {
        if case .active(let at) = phase { hover = at } else { hover = nil }
    }

    private var brushRadius: CGFloat {
        CGFloat(model.editor.brushSize) * max(rect.width, rect.height)
    }

    private func norm(_ p: CGPoint) -> (x: Double, y: Double)? {
        guard let frame, rect.width > 0, rect.height > 0 else { return nil }
        return frame.norm(u: (p.x - rect.minX) / rect.width, v: (p.y - rect.minY) / rect.height)
    }

    private func paint(_ brush: String) -> some Gesture {
        DragGesture(minimumDistance: 0)
            .onChanged { g in
                if trail.isEmpty { erasing = optionDown }
                if trail.last.map({ hypot($0.x - g.location.x, $0.y - g.location.y) > 2 }) ?? true {
                    trail.append(g.location)
                }
            }
            .onEnded { _ in
                let pts: [JSONValue] = trail.compactMap { p in norm(p).map { [.double($0.x), .double($0.y)] } }
                trail = []
                let mode = erasing ? "erase" : brush
                erasing = false
                guard !pts.isEmpty else { return }
                let e = model.editor
                model.perform("lensBlur.brushStroke", [
                    "mode": .string(mode), "points": .array(pts), "size": .double(e.brushSize), "feather": .double(e.brushFeather),
                    "flow": .double(e.brushFlow), "autoMask": .bool(e.brushAutoMask),
                ])
            }
    }
}
