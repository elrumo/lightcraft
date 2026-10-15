import LightCraftCore
import SwiftUI
import UniformTypeIdentifiers

/// The Tone Curve: the parametric curve's sliders, or a point curve (master, red, green, blue) to
/// drag — a tap adds a point, a double tap on one takes it away — and the curve presets.
struct CurveEditor: View {
    @Environment(AppModel.self) private var model
    @State private var channel = "parametric"
    @State private var presets: [String] = []
    @State private var saving = false
    @State private var presetName = ""
    @State private var importing = false
    @State private var exported: SharedFile?

    var body: some View {
        HStack {
            Picker("Channel", selection: $channel) {
                Image(systemName: "slider.horizontal.below.square.filled.and.square").tag("parametric").accessibilityLabel("Parametric")
                Image(systemName: "circle.fill").tag("master").accessibilityLabel("Point Curve")
                Image(systemName: "circle.fill").foregroundStyle(.red).tag("red").accessibilityLabel("Red")
                Image(systemName: "circle.fill").foregroundStyle(.green).tag("green").accessibilityLabel("Green")
                Image(systemName: "circle.fill").foregroundStyle(.blue).tag("blue").accessibilityLabel("Blue")
            }
            .pickerStyle(.segmented)
            Menu {
                ForEach(presets, id: \.self) { name in
                    Button(name) { model.perform("curve.applyPreset", ["name": .string(name)]) }
                }
                Divider()
                Button("Save Point Curve as Preset…", systemImage: "plus") {
                    presetName = ""
                    saving = true
                }
                Button("Import Curve Presets…", systemImage: "square.and.arrow.down") { importing = true }
                Button("Export Curve Presets…", systemImage: "square.and.arrow.up") { Task { await export() } }
                Divider()
                Button("Reset Curve", systemImage: "arrow.counterclockwise") { model.perform("curve.reset", ["channel": .string(channel == "parametric" ? "parametric" : channel)]) }
            } label: {
                Image(systemName: "ellipsis.circle")
            }
            .accessibilityLabel("Curve Presets")
            .alert("Save Curve Preset", isPresented: $saving) {
                TextField("Preset Name", text: $presetName)
                Button("Save") {
                    Task {
                        await model.run("curve.savePreset", ["name": .string(presetName.trimmingCharacters(in: .whitespaces))])
                        await loadPresets()
                    }
                }
                Button("Cancel", role: .cancel) {}
            }
            .fileImporter(isPresented: $importing, allowedContentTypes: [.json, UTType(filenameExtension: "lccurve") ?? .json], allowsMultipleSelection: true) { result in
                guard case .success(let urls) = result else { return }
                let paths = PresetFiles.stage(urls)
                Task {
                    if let r = await model.run("curve.importPresets", ["paths": .array(paths.map { .string($0) })]) {
                        model.show(String(localized: "Imported \(r["imported"].array?.count ?? 0) curve presets"))
                    }
                    await loadPresets()
                }
            }
            .sheet(item: $exported) { file in
                ActivityView(urls: [file.url]) { exported = nil }
            }
        }
        if channel == "parametric" {
            Sliders(section: "curve")
        } else {
            PointCurve(channel: channel)
                .aspectRatio(1, contentMode: .fit)
                .frame(maxWidth: 300)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 6)
        }
        EmptyView()
            .task { await loadPresets() }
    }

    private func loadPresets() async {
        guard let engine = model.engine, let r = try? await engine.execute("curve.presets") else { return }
        presets = r["presets"].array?.compactMap { $0["name"].string } ?? []
    }

    /// The user's curve presets as a .lccurve file, to share or save.
    private func export() async {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("LightCraft Curves.lccurve")
        if await model.run("curve.exportPresets", ["path": .string(url.path)]) != nil {
            exported = SharedFile(url: url)
        }
    }
}

/// A point curve on a square: points to drag.
private struct PointCurve: View {
    @Environment(AppModel.self) private var model
    let channel: String
    @State private var points: [CGPoint] = []
    @State private var dragging: Int?
    @State private var taps = DoubleTap()
    /// The curve's values at even steps (`curve.sample`): what the pipeline applies.
    @State private var samples: [Double] = []

    var body: some View {
        GeometryReader { geo in
            let size = geo.size
            let toView = { (p: CGPoint) in CGPoint(x: p.x * size.width, y: (1 - p.y) * size.height) }
            ZStack {
                Rectangle().fill(Color.lcTertiaryFill)
                Path { path in
                    for i in 1..<4 {
                        let t = CGFloat(i) / 4
                        path.move(to: CGPoint(x: t * size.width, y: 0))
                        path.addLine(to: CGPoint(x: t * size.width, y: size.height))
                        path.move(to: CGPoint(x: 0, y: t * size.height))
                        path.addLine(to: CGPoint(x: size.width, y: t * size.height))
                    }
                }
                .stroke(.secondary.opacity(0.3), lineWidth: 0.5)
                Path { path in
                    path.move(to: CGPoint(x: 0, y: size.height))
                    path.addLine(to: CGPoint(x: size.width, y: 0))
                }
                .stroke(.secondary.opacity(0.4), style: StrokeStyle(lineWidth: 1, dash: [3, 3]))
                Path { path in
                    guard samples.count > 1 else { return }
                    for (i, y) in samples.enumerated() {
                        let p = toView(CGPoint(x: Double(i) / Double(samples.count - 1), y: y))
                        if i == 0 { path.move(to: p) } else { path.addLine(to: p) }
                    }
                }
                .stroke(color, lineWidth: 2)
                ForEach(Array(shown.enumerated()), id: \.offset) { i, p in
                    Circle()
                        .fill(dragging == i ? color : .white)
                        .stroke(color, lineWidth: 2)
                        .frame(width: 14, height: 14)
                        .position(toView(p))
                }
            }
            .contentShape(Rectangle())
            .gesture(drag(size))
        }
        .clipShape(.rect(cornerRadius: 8))
        .onAppear(perform: read)
        .onChange(of: model.state.catalogRevision) { if dragging == nil { read() } }
        .onChange(of: channel) { read() }
        .task(id: SampleKey(channel: channel, revision: model.state.catalogRevision)) {
            guard let engine = model.engine, let r = try? await engine.execute("curve.sample", ["channel": .string(channel), "n": 96]) else { return }
            samples = r["samples"].array?.compactMap(\.double) ?? []
        }
        .accessibilityElement()
        .accessibilityLabel("\(channel.capitalized) curve")
        .accessibilityValue("\(shown.count) points")
    }

    private nonisolated struct SampleKey: Hashable {
        let channel: String
        let revision: UInt64
    }

    private var color: Color {
        switch channel {
        case "red": .red
        case "green": .green
        case "blue": .blue
        default: .primary
        }
    }

    /// The points, with the corners when the curve is straight.
    private var shown: [CGPoint] {
        points.isEmpty ? [CGPoint(x: 0, y: 0), CGPoint(x: 1, y: 1)] : points
    }

    private func read() {
        let raw = model.editor.setting("curve", channel).array ?? []
        points = raw.compactMap { v in
            if let x = v["x"].double, let y = v["y"].double { return CGPoint(x: x, y: y) }
            if let a = v.array, a.count == 2, let x = a[0].double, let y = a[1].double { return CGPoint(x: x, y: y) }
            return nil
        }
    }

    private func nearest(_ p: CGPoint, _ size: CGSize) -> Int? {
        let reach = 24 / max(size.width, 1)
        return shown.enumerated().min { hypot($0.element.x - p.x, $0.element.y - p.y) < hypot($1.element.x - p.x, $1.element.y - p.y) }
            .flatMap { hypot($0.element.x - p.x, $0.element.y - p.y) < reach ? $0.offset : nil }
    }

    private func drag(_ size: CGSize) -> some Gesture {
        DragGesture(minimumDistance: 0)
            .onChanged { g in
                let p = CGPoint(x: min(max(g.location.x / size.width, 0), 1), y: min(max(1 - g.location.y / size.height, 0), 1))
                var pts = shown
                if dragging == nil {
                    let start = CGPoint(x: g.startLocation.x / size.width, y: 1 - g.startLocation.y / size.height)
                    if let i = nearest(start, size) {
                        dragging = i
                    } else {
                        pts.append(p)
                        pts.sort { $0.x < $1.x }
                        dragging = pts.firstIndex(of: p)
                    }
                    model.engine?.send("engine.execute", ["command": "develop.beginInteraction", "params": ["label": "Tone Curve"]])
                }
                guard let i = dragging, i < pts.count else { return }
                // a point stays between its neighbours
                let lo = i > 0 ? pts[i - 1].x + 0.01 : 0
                let hi = i < pts.count - 1 ? pts[i + 1].x - 0.01 : 1
                pts[i] = CGPoint(x: min(max(p.x, lo), hi), y: p.y)
                points = pts
                send(pts)
                model.requestDraft()
            }
            .onEnded { g in
                model.engine?.send("engine.execute", ["command": "develop.endInteraction"])
                dragging = nil
                // a double tap on a point takes it away (the drag takes the touch at touch-down, so a
                // tap gesture beside it never fires)
                guard taps.ended(g) else { return }
                let p = CGPoint(x: g.location.x / size.width, y: 1 - g.location.y / size.height)
                if let i = nearest(p, size), shown.count > 2 {
                    var pts = shown
                    pts.remove(at: i)
                    commit(pts)
                }
            }
    }

    private func send(_ pts: [CGPoint]) {
        let arr: JSONValue = .array(pts.map { .array([.double(Double($0.x)), .double(Double($0.y))]) })
        model.engine?.send("engine.execute", ["command": "develop.curve", "params": ["channel": .string(channel), "points": arr]])
    }

    private func commit(_ pts: [CGPoint]) {
        points = pts
        let arr: JSONValue = .array(pts.map { .array([.double(Double($0.x)), .double(Double($0.y))]) })
        model.perform("develop.curve", ["channel": .string(channel), "points": arr])
    }
}
