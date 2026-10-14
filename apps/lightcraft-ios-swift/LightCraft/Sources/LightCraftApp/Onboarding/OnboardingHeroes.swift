#if os(iOS)
import SwiftUI

/// The app icon's three colours (`assets/app-icon/README.md`), which the welcome is drawn in.
enum Brand {
    static let ink = Color(red: 11 / 255, green: 11 / 255, blue: 12 / 255)
    static let paper = Color(red: 239 / 255, green: 233 / 255, blue: 220 / 255)
    static let amber = Color(red: 242 / 255, green: 165 / 255, blue: 22 / 255)
}

/// The pictures of the welcome's pages. Each is drawn in a 340 × 330 box, plays when its page arrives
/// (`active`) and, unless Reduce Motion is on, keeps moving a little while it is there.
enum OnboardingHero {
    case lynx, fan, grade, devices
}

// MARK: 1 · the lynx

/// The app icon, lit by a breathing glow, turning a few degrees with a sheen crossing it.
struct LynxHero: View {
    let active: Bool
    let reduceMotion: Bool
    @State private var shown = false

    /// The lynx from the asset catalog (the Xcode build); a drawn mark when it isn't bundled (xtool).
    private var icon: some View {
        Group {
            if let image = UIImage(named: "Lynx") {
                Image(uiImage: image).resizable().scaledToFit()
            } else {
                Image(systemName: "camera.aperture")
                    .font(.system(size: 80))
                    .foregroundStyle(Brand.ink)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(Brand.amber, in: .rect(cornerRadius: 40, style: .continuous))
            }
        }
        .frame(width: 184, height: 184)
    }

    var body: some View {
        TimelineView(.animation(paused: !active || reduceMotion)) { context in
            let t = reduceMotion ? 0 : context.date.timeIntervalSinceReferenceDate
            ZStack {
                Circle()
                    .fill(RadialGradient(colors: [Brand.amber.opacity(0.5), .clear], center: .center, startRadius: 0, endRadius: 170))
                    .frame(width: 340, height: 340)
                    .scaleEffect(1 + 0.05 * sin(t * 1.4))
                icon
                    .overlay {
                        // a band of light, across the icon every few seconds
                        let sweep = (t * 0.28).truncatingRemainder(dividingBy: 1)
                        LinearGradient(colors: [.clear, .white.opacity(0.35), .clear], startPoint: .leading, endPoint: .trailing)
                            .frame(width: 80)
                            .rotationEffect(.degrees(18))
                            .offset(x: -170 + sweep * 460)
                            .mask { icon }
                    }
                    .shadow(color: Brand.amber.opacity(0.4), radius: 36, y: 22)
                    .rotation3DEffect(.degrees(sin(t * 0.8) * 5), axis: (x: 0, y: 1, z: 0), perspective: 0.5)
                    .rotation3DEffect(.degrees(cos(t * 0.6) * 3), axis: (x: 1, y: 0, z: 0), perspective: 0.5)
                    .offset(y: sin(t * 1.1) * 6)
            }
        }
        .scaleEffect(shown ? 1 : 0.6)
        .blur(radius: shown ? 0 : 14)
        .opacity(shown ? 1 : 0)
        .onChange(of: active, initial: true) { _, now in
            guard now, !shown else { return }
            withAnimation(reduceMotion ? .easeOut(duration: 0.4) : .spring(response: 0.95, dampingFraction: 0.68)) { shown = true }
        }
    }
}

// MARK: 2 · import

/// Contact prints fanning out, and where they come from.
struct FanHero: View {
    let active: Bool
    let reduceMotion: Bool
    @State private var shown = false

    private struct Print {
        let scene: Landscape
        let angle: Double
        let x: CGFloat
        let y: CGFloat
        let stars: Int
    }

    private static let prints = [
        Print(scene: .dusk, angle: -14, x: -84, y: 22, stars: 0),
        Print(scene: .dawn, angle: -7, x: -42, y: 6, stars: 3),
        Print(scene: .meadow, angle: 0, x: 0, y: 0, stars: 5),
        Print(scene: .desert, angle: 7, x: 42, y: 6, stars: 0),
        Print(scene: .night, angle: 14, x: 84, y: 22, stars: 4),
    ]

    var body: some View {
        ZStack {
            ForEach(Self.prints.indices, id: \.self) { i in
                let print = Self.prints[i]
                card(print)
                    .rotationEffect(.degrees(shown ? print.angle : 0), anchor: .bottom)
                    .offset(x: shown ? print.x : 0, y: (shown ? print.y : 70) - 62)
                    .scaleEffect(shown ? 1 : 0.82)
                    .opacity(shown ? 1 : 0)
                    .zIndex(Double(-abs(i - 2)))
                    .animation(reduceMotion ? .easeOut(duration: 0.3) : .spring(response: 0.8, dampingFraction: 0.66).delay(0.07 * Double(i)), value: shown)
            }
            GlassEffectContainer(spacing: 10) {
                VStack(spacing: 10) {
                    HStack(spacing: 10) {
                        chip("Photos", symbol: "photo.on.rectangle", order: 0)
                        chip("Files", symbol: "folder", order: 1)
                    }
                    HStack(spacing: 10) {
                        chip("Camera", symbol: "camera", order: 2)
                        chip("Lightroom", symbol: "books.vertical", order: 3)
                    }
                }
            }
            .offset(y: 118)
        }
        .onChange(of: active, initial: true) { _, now in
            if now { shown = true }
        }
    }

    /// A print: paper border, the picture, a few stars on the margin.
    private func card(_ print: Print) -> some View {
        VStack(spacing: 0) {
            print.scene
                .frame(width: 110, height: 124)
                .clipShape(.rect(cornerRadius: 2))
            HStack(spacing: 2) {
                ForEach(0..<print.stars, id: \.self) { _ in
                    Image(systemName: "star.fill").font(.system(size: 7)).foregroundStyle(Brand.amber)
                }
            }
            .frame(height: 24)
        }
        .padding([.horizontal, .top], 7)
        .background(Brand.paper, in: .rect(cornerRadius: 5))
        .shadow(color: .black.opacity(0.5), radius: 14, y: 8)
    }

    private func chip(_ title: LocalizedStringKey, symbol: String, order: Int) -> some View {
        Label(title, systemImage: symbol)
            .font(.subheadline.weight(.medium))
            .foregroundStyle(Brand.paper)
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .glassEffect(.regular, in: .capsule)
            .opacity(shown ? 1 : 0)
            .offset(y: shown ? 0 : 14)
            .animation(reduceMotion ? .easeOut(duration: 0.3) : .snappy.delay(0.45 + 0.08 * Double(order)), value: shown)
    }
}

// MARK: 3 · edit

/// A print graded before your eyes: a divider sweeps over it, and the tools' sliders follow.
struct GradeHero: View {
    let active: Bool
    let reduceMotion: Bool
    @State private var shown = false

    private let width: CGFloat = 304
    private let height: CGFloat = 206

    var body: some View {
        TimelineView(.animation(paused: !active || reduceMotion)) { context in
            let t = reduceMotion ? 0 : context.date.timeIntervalSinceReferenceDate
            // the divider's place across the print (0…1); the edit shows to the right of it
            let x = 0.5 + 0.4 * sin(t * 1.1)
            VStack(spacing: 16) {
                photo(x: x)
                tools(applied: 1 - x)
            }
        }
        .scaleEffect(shown ? 1 : 0.92)
        .offset(y: shown ? 0 : 26)
        .opacity(shown ? 1 : 0)
        .onChange(of: active, initial: true) { _, now in
            guard now, !shown else { return }
            withAnimation(reduceMotion ? .easeOut(duration: 0.4) : .spring(response: 0.8, dampingFraction: 0.75)) { shown = true }
        }
    }

    private func photo(x: Double) -> some View {
        ZStack(alignment: .leading) {
            Landscape.dawn
            // before: flat and pale, left of the divider
            Landscape.dawn
                .saturation(0.2)
                .contrast(0.78)
                .brightness(0.05)
                .mask(alignment: .leading) { Rectangle().frame(width: width * x) }
            Rectangle()
                .fill(.white)
                .frame(width: 2)
                .shadow(color: .black.opacity(0.4), radius: 3)
                .offset(x: width * x - 1)
            Image(systemName: "arrow.left.and.right")
                .font(.system(size: 12, weight: .bold))
                .foregroundStyle(Brand.ink)
                .frame(width: 34, height: 34)
                .background(Brand.paper, in: .circle)
                .shadow(color: .black.opacity(0.35), radius: 6, y: 2)
                .offset(x: width * x - 17)
        }
        .frame(width: width, height: height)
        .clipShape(.rect(cornerRadius: 2))
        .overlay(alignment: .topLeading) { badge("Before").padding(8).opacity(x > 0.2 ? 1 : 0) }
        .overlay(alignment: .topTrailing) { badge("After").padding(8).opacity(x < 0.8 ? 1 : 0) }
        .padding(8)
        .background(Brand.paper, in: .rect(cornerRadius: 6))
        .shadow(color: .black.opacity(0.5), radius: 16, y: 10)
    }

    /// ("tag" is a View modifier: a helper of that name would be passed over for it.)
    private func badge(_ title: LocalizedStringKey) -> some View {
        Text(title)
            .font(.caption2.weight(.bold))
            .textCase(.uppercase)
            .foregroundStyle(.white)
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(.black.opacity(0.45), in: .capsule)
    }

    /// Three sliders, set further as more of the edit shows.
    private func tools(applied a: Double) -> some View {
        VStack(spacing: 12) {
            slider("Exposure", value: 0.5 + 0.3 * a, range: 4, digits: 1)
            slider("Contrast", value: 0.5 + 0.34 * a, range: 200, digits: 0)
            slider("Temperature", value: 0.5 + 0.2 * a, range: 100, digits: 0)
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 16)
        .frame(width: width + 16)
        .background(.white.opacity(0.07), in: .rect(cornerRadius: 20))
        .overlay(RoundedRectangle(cornerRadius: 20).strokeBorder(.white.opacity(0.1), lineWidth: 0.75))
    }

    private func slider(_ title: LocalizedStringKey, value v: Double, range: Double, digits: Int) -> some View {
        HStack(spacing: 12) {
            Text(title)
                .font(.caption)
                .foregroundStyle(Brand.paper.opacity(0.7))
                .lineLimit(1)
                .frame(width: 84, alignment: .leading)
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule().fill(Brand.paper.opacity(0.18)).frame(height: 3)
                    Capsule().fill(Brand.amber).frame(width: geo.size.width * v, height: 3)
                    Circle().fill(Brand.paper).frame(width: 16, height: 16)
                        .shadow(color: .black.opacity(0.4), radius: 3, y: 1)
                        .offset(x: geo.size.width * v - 8)
                }
                .frame(height: geo.size.height)
            }
            .frame(height: 16)
            Text(verbatim: String(format: "%+.\(digits)f", (v - 0.5) * range))
                .font(.caption2.monospacedDigit())
                .foregroundStyle(Brand.paper.opacity(0.6))
                .frame(width: 34, alignment: .trailing)
        }
    }
}

// MARK: 4 · yours

/// Your devices on an orbit around a lock; a pulse goes out from it.
struct DevicesHero: View {
    let active: Bool
    let reduceMotion: Bool
    @State private var shown = false

    private static let devices = ["iphone", "ipad", "server.rack"]

    var body: some View {
        TimelineView(.animation(paused: !active || reduceMotion)) { context in
            let t = reduceMotion ? 0 : context.date.timeIntervalSinceReferenceDate
            ZStack {
                Circle()
                    .fill(RadialGradient(colors: [Brand.amber.opacity(0.4), .clear], center: .center, startRadius: 0, endRadius: 170))
                    .frame(width: 340, height: 340)
                ForEach([62.0, 108.0, 154.0], id: \.self) { r in
                    Circle().strokeBorder(Brand.paper.opacity(0.13), lineWidth: 1).frame(width: r * 2, height: r * 2)
                }
                // a pulse, from the lock out to the edge, every three seconds
                let pulse = (t / 3).truncatingRemainder(dividingBy: 1)
                Circle()
                    .strokeBorder(Brand.amber.opacity(0.7 * (1 - pulse)), lineWidth: 2)
                    .frame(width: 100 + pulse * 220, height: 100 + pulse * 220)
                ForEach(Self.devices.indices, id: \.self) { i in
                    let a: Double = t * 0.35 + Double(i) * (2 * Double.pi / 3) - Double.pi / 2
                    Image(systemName: Self.devices[i])
                        .font(.system(size: 20, weight: .medium))
                        .foregroundStyle(Brand.paper)
                        .frame(width: 54, height: 54)
                        .background(Brand.paper.opacity(0.12), in: .circle)
                        .overlay(Circle().strokeBorder(Brand.paper.opacity(0.28), lineWidth: 0.75))
                        .offset(x: 108 * cos(a), y: 108 * sin(a))
                }
                Image(systemName: "lock.fill")
                    .font(.system(size: 38, weight: .semibold))
                    .foregroundStyle(Brand.ink)
                    .frame(width: 100, height: 100)
                    .background(Brand.amber, in: .circle)
                    .shadow(color: Brand.amber.opacity(0.5), radius: 26, y: 10)
            }
        }
        .scaleEffect(shown ? 1 : 0.7)
        .opacity(shown ? 1 : 0)
        .onChange(of: active, initial: true) { _, now in
            guard now, !shown else { return }
            withAnimation(reduceMotion ? .easeOut(duration: 0.4) : .spring(response: 0.9, dampingFraction: 0.7)) { shown = true }
        }
    }
}

// MARK: pictures

/// A picture painted in code: a sky, a sun or moon, and rolling ridges. The prints' and the grade's photos
/// (original work, like `crates/scenes`' demo photographs).
struct Landscape: View {
    let sky: [Color]
    let sun: Color
    let sunAt: CGPoint
    let ridges: [Color]
    var stars = false
    var seed = 0.0

    static let dusk = Landscape(
        sky: [Color(hex: 0x2B1B4D), Color(hex: 0xB3476B), Color(hex: 0xF6A15A)], sun: Color(hex: 0xFFE2A8), sunAt: CGPoint(x: 0.62, y: 0.6),
        ridges: [Color(hex: 0x5A2D5E), Color(hex: 0x3A1D45), Color(hex: 0x1E1028)], seed: 0.4)
    static let dawn = Landscape(
        sky: [Color(hex: 0x1F3A5F), Color(hex: 0x6C8EBF), Color(hex: 0xF2C6A0)], sun: Color(hex: 0xFFF1D0), sunAt: CGPoint(x: 0.3, y: 0.56),
        ridges: [Color(hex: 0x5C7699), Color(hex: 0x3A4F6E), Color(hex: 0x1E2B40)], seed: 1.3)
    static let night = Landscape(
        sky: [Color(hex: 0x050816), Color(hex: 0x0F1D3D), Color(hex: 0x23406B)], sun: Color(hex: 0xE8EEF8), sunAt: CGPoint(x: 0.74, y: 0.26),
        ridges: [Color(hex: 0x0E1A33), Color(hex: 0x0A1226), Color(hex: 0x060A18)], stars: true, seed: 2.1)
    static let meadow = Landscape(
        sky: [Color(hex: 0x79B7E8), Color(hex: 0xBFE3F2), Color(hex: 0xF3F0D2)], sun: Color(hex: 0xFFFBE0), sunAt: CGPoint(x: 0.5, y: 0.34),
        ridges: [Color(hex: 0x7FB069), Color(hex: 0x4E8A4A), Color(hex: 0x2F5E37)], seed: 3.2)
    static let desert = Landscape(
        sky: [Color(hex: 0xF4B66A), Color(hex: 0xF7D9A0), Color(hex: 0xFBEBC6)], sun: Color(hex: 0xFFF6DA), sunAt: CGPoint(x: 0.8, y: 0.3),
        ridges: [Color(hex: 0xD98E4A), Color(hex: 0xB86A33), Color(hex: 0x8B4A25)], seed: 4.6)

    var body: some View {
        Canvas { ctx, size in
            let w = size.width, h = size.height
            ctx.fill(Path(CGRect(origin: .zero, size: size)),
                     with: .linearGradient(Gradient(colors: sky), startPoint: .zero, endPoint: CGPoint(x: 0, y: h)))
            if stars {
                for k in 0..<46 {
                    let r = 0.4 + 1.1 * Self.unit(k, 3)
                    ctx.fill(Path(ellipseIn: CGRect(x: w * Self.unit(k, 1) - r, y: h * 0.6 * Self.unit(k, 2) - r, width: r * 2, height: r * 2)),
                             with: .color(.white.opacity(0.35 + 0.5 * Self.unit(k, 4))))
                }
            }
            let c = CGPoint(x: w * sunAt.x, y: h * sunAt.y)
            let r = w * 0.07
            ctx.fill(Path(ellipseIn: CGRect(x: c.x - r * 4, y: c.y - r * 4, width: r * 8, height: r * 8)),
                     with: .radialGradient(Gradient(colors: [sun.opacity(0.65), .clear]), center: c, startRadius: 0, endRadius: r * 4))
            ctx.fill(Path(ellipseIn: CGRect(x: c.x - r, y: c.y - r, width: r * 2, height: r * 2)), with: .color(sun))
            for (i, color) in ridges.enumerated() {
                let base = h * (0.58 + 0.1 * Double(i))
                let amp = h * (0.17 - 0.03 * Double(i))
                let f = 2.4 + Double(i) * 1.3
                let phase = seed + Double(i) * 1.7
                var path = Path()
                path.move(to: CGPoint(x: 0, y: h))
                for x in stride(from: 0.0, through: w, by: 3) {
                    let u = x / w * .pi
                    let hills = 0.55 * sin(u * f + phase) + 0.3 * sin(u * f * 2.3 + phase * 1.9) + 0.15 * sin(u * f * 5.1 + phase * 3)
                    path.addLine(to: CGPoint(x: x, y: base - amp * hills))
                }
                path.addLine(to: CGPoint(x: w, y: h))
                path.closeSubpath()
                ctx.fill(path, with: .color(color))
            }
        }
    }

    /// A repeatable pseudo-random number in 0…1.
    private static func unit(_ k: Int, _ salt: Double) -> Double {
        let v = sin(Double(k) * 12.9898 + salt * 78.233) * 43758.5453
        return v - v.rounded(.down)
    }
}

private extension Color {
    init(hex: UInt32) {
        self.init(red: Double((hex >> 16) & 0xFF) / 255, green: Double((hex >> 8) & 0xFF) / 255, blue: Double(hex & 0xFF) / 255)
    }
}
#endif
