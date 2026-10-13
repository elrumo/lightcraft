#if os(iOS)
import LightCraftCore
import SwiftUI

/// The Tone Curve: the parametric curve's sliders, or a point curve (master, red, green, blue) to
/// drag — a tap adds a point, a double tap on one takes it away — and the curve presets.
struct CurveEditor: View {
    @Environment(AppModel.self) private var model
    @State private var channel = "parametric"
    @State private var presets: [String] = []

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
                Button("Reset Curve", systemImage: "arrow.counterclockwise") { model.perform("curve.reset", ["channel": .string(channel == "parametric" ? "parametric" : channel)]) }
            } label: {
                Image(systemName: "ellipsis.circle")
            }
            .accessibilityLabel("Curve Presets")
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
            .task {
                guard let engine = model.engine, let r = try? await engine.execute("curve.presets") else { return }
                presets = r["presets"].array?.compactMap { $0["name"].string } ?? []
            }
    }
}

/// A point curve on a square: points to drag.
private struct PointCurve: View {
    @Environment(AppModel.self) private var model
    let channel: String
    @State private var points: [CGPoint] = []
    @State private var dragging: Int?
    /// The curve's values at even steps (`curve.sample`): what the pipeline applies.
    @State private var samples: [Double] = []

    var body: some View {
        GeometryReader { geo in
            let size = geo.size
            let toView = { (p: CGPoint) in CGPoint(x: p.x * size.width, y: (1 - p.y) * size.height) }
            ZStack {
                Rectangle().fill(Color(uiColor: .tertiarySystemFill))
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
            .onTapGesture(count: 2) { location in
                let p = CGPoint(x: location.x / size.width, y: 1 - location.y / size.height)
                if let i = nearest(p, size), shown.count > 2 {
                    var pts = shown
                    pts.remove(at: i)
                    commit(pts)
                }
            }
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
            .onEnded { _ in
                model.engine?.send("engine.execute", ["command": "develop.endInteraction"])
                dragging = nil
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

/// Point Color: the colours picked from the photo (the eyedropper, then a tap on the photo), and for
/// the chosen one its hue, saturation and luminance shifts and how wide a range it takes.
struct PointColorView: View {
    @Environment(AppModel.self) private var model
    @State private var chosen = 0
    @State private var swatches: [String] = []

    var body: some View {
        let editor = model.editor
        HStack(spacing: 10) {
            Button {
                editor.picking = .pointColor
                model.show(String(localized: "Tap a colour in the photo"))
            } label: {
                Image(systemName: "eyedropper").frame(width: 38, height: 38)
            }
            .buttonStyle(.bordered)
            .disabled(swatches.count >= 8)
            .accessibilityLabel("Pick a Color")
            ForEach(Array(swatches.enumerated()), id: \.offset) { i, hex in
                Button {
                    chosen = i
                } label: {
                    Circle()
                        .fill(Color(hex: hex) ?? .gray)
                        .frame(width: 32, height: 32)
                        .overlay { if chosen == i { Circle().strokeBorder(.primary, lineWidth: 2).padding(-4) } }
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Color \(i + 1)")
            }
            Spacer()
            if chosen < swatches.count {
                Button(role: .destructive) {
                    model.perform("pointColor.delete", ["index": .int(Int64(chosen))])
                    chosen = 0
                } label: {
                    Image(systemName: "trash")
                }
                .accessibilityLabel("Delete Color")
            }
        }
        .task(id: model.state.catalogRevision) {
            guard let engine = model.engine, let list = try? await engine.execute("pointColor.list") else { return }
            swatches = list.array?.compactMap { $0["swatch"].string } ?? []
        }
        if chosen < swatches.count {
            Sliders(section: "pointColor") { $0.id.hasPrefix("pointColor.\(chosen).") }
        } else {
            Text("Pick a colour in the photo to adjust it everywhere it appears.")
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
    }
}
#endif
