#if os(iOS)
import SwiftUI

/// The first launch's welcome: four pages on an ink canvas lit in the app icon's amber, then Get Started.
/// `Prefs.onboarded` remembers it was seen. Each page is a picture (`OnboardingHeroes.swift`) over its words;
/// the pictures play when their page arrives and the backdrop follows the swipe.
struct OnboardingView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var selection: Int? = 0
    /// The scroll position in pages (0…3, fractions while swiping): the backdrop and the parallax follow it.
    @State private var progress: CGFloat = 0

    fileprivate struct Page {
        let hero: OnboardingHero
        let title: LocalizedStringKey
        let text: LocalizedStringKey
    }

    private static let pages = [
        Page(hero: .lynx, title: "Welcome to LightCraft",
             text: "A photo library and raw developer. Free and open source."),
        Page(hero: .fan, title: "Bring in your photos",
             text: "Add them from Photos, Files, a camera or card, or a Lightroom catalog with the + button. Albums, ratings, keywords and People keep them in order."),
        Page(hero: .grade, title: "Edit without limits",
             text: "Develop raw photos, mask, remove distractions. Your originals are never changed, and every edit can be undone."),
        Page(hero: .devices, title: "Yours, on your devices",
             text: "Your photos stay on this device. Syncing with your own server is optional, and LightCraft asks for access only when it needs it."),
    ]

    var body: some View {
        let page = selection ?? 0
        let last = page == Self.pages.count - 1
        ZStack {
            Backdrop(progress: progress, animated: !reduceMotion)
            ScrollView(.horizontal) {
                HStack(spacing: 0) {
                    ForEach(Self.pages.indices, id: \.self) { i in
                        PageView(page: Self.pages[i], index: i, progress: progress, active: page == i, reduceMotion: reduceMotion)
                            .containerRelativeFrame(.horizontal)
                            .id(i)
                    }
                }
                .scrollTargetLayout()
            }
            .scrollTargetBehavior(.paging)
            .scrollPosition(id: $selection)
            .scrollIndicators(.hidden)
            .onScrollGeometryChange(for: CGFloat.self) { $0.contentOffset.x / max($0.containerSize.width, 1) } action: { _, new in
                progress = new
            }
        }
        .safeAreaInset(edge: .bottom) { controls(page: page, last: last) }
        .overlay(alignment: .topTrailing) {
            Button("Skip", action: finish)
                .buttonStyle(.glass)
                .foregroundStyle(Brand.paper)
                .padding(.horizontal, 16)
                .opacity(last ? 0 : 1)
                .allowsHitTesting(!last)
                .animation(.snappy, value: last)
        }
        .background(Brand.ink.ignoresSafeArea())
        .preferredColorScheme(.dark)
        .haptic(.selection, trigger: page)
    }

    /// The page dots (the current one stretches) and the button.
    private func controls(page: Int, last: Bool) -> some View {
        VStack(spacing: 22) {
            HStack(spacing: 8) {
                ForEach(Self.pages.indices, id: \.self) { i in
                    Capsule()
                        .fill(i == page ? Brand.amber : Brand.paper.opacity(0.28))
                        .frame(width: i == page ? 26 : 7, height: 7)
                }
            }
            .animation(.snappy, value: page)
            .accessibilityElement()
            .accessibilityLabel(Text(verbatim: "\(page + 1) / \(Self.pages.count)"))
            Button {
                if last {
                    finish()
                } else {
                    withAnimation(.snappy) { selection = page + 1 }
                }
            } label: {
                ZStack {
                    if last {
                        Text("Get Started").transition(.blurReplace)
                    } else {
                        Text("Continue").transition(.blurReplace)
                    }
                }
                .animation(.snappy, value: last)
            }
            .buttonStyle(AmberButton())
        }
        .frame(maxWidth: 420)
        .padding(.horizontal, 28)
        .padding(.top, 8)
        .padding(.bottom, 12)
        .frame(maxWidth: .infinity)
        .background(alignment: .bottom) {
            LinearGradient(colors: [Brand.ink.opacity(0), Brand.ink.opacity(0.92)], startPoint: .top, endPoint: UnitPoint(x: 0.5, y: 0.35))
                .padding(.top, -40)
                .ignoresSafeArea(edges: .bottom)
                .allowsHitTesting(false)
        }
    }

    private func finish() {
        model.prefs.onboarded = true
        model.cover = nil
    }
}

/// One page: its picture, parallaxed against the swipe, over its words.
private struct PageView: View {
    let page: OnboardingView.Page
    let index: Int
    let progress: CGFloat
    let active: Bool
    let reduceMotion: Bool

    var body: some View {
        GeometryReader { geo in
            // −1…1: where this page is relative to the screen
            let p = progress - CGFloat(index)
            // the pictures are drawn in a 340 × 330 box, shrunk to fit short screens
            let fit = max(0.3, min(1, geo.size.height * 0.5 / 330, (geo.size.width - 24) / 340))
            ScrollView {
                VStack(spacing: 28) {
                    hero
                        .frame(width: 340, height: 330)
                        .scaleEffect(fit)
                        .frame(width: 340 * fit, height: 330 * fit)
                        .accessibilityHidden(true)
                        .offset(x: p * geo.size.width * 0.35)
                        .opacity(1 - min(1, abs(p) * 1.3))
                    words
                        .padding(.horizontal, 28)
                        .offset(x: p * geo.size.width * 0.12)
                        .opacity(1 - min(1, abs(p) * 1.8))
                }
                .padding(.vertical, 28)
                .frame(maxWidth: .infinity, minHeight: geo.size.height)
            }
            .scrollBounceBehavior(.basedOnSize)
        }
    }

    @ViewBuilder private var hero: some View {
        switch page.hero {
        case .lynx: LynxHero(active: active, reduceMotion: reduceMotion)
        case .fan: FanHero(active: active, reduceMotion: reduceMotion)
        case .grade: GradeHero(active: active, reduceMotion: reduceMotion)
        case .devices: DevicesHero(active: active, reduceMotion: reduceMotion)
        }
    }

    private var words: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(page.title)
                .font(.largeTitle.bold())
                .foregroundStyle(Brand.paper)
                .fixedSize(horizontal: false, vertical: true)
                .accessibilityAddTraits(.isHeader)
            Text(page.text)
                .font(.body)
                .foregroundStyle(Brand.paper.opacity(0.74))
                .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: 520, alignment: .leading)
        .frame(maxWidth: .infinity)
    }
}

/// The canvas: ink with a glow of amber that moves from page to page, drifting a little (a mesh gradient),
/// and darker under the words.
private struct Backdrop: View {
    let progress: CGFloat
    let animated: Bool

    /// How much amber each of the mesh's nine points has, per page (rows top to bottom): the light sits
    /// behind the picture and leaves the bottom dark for the words.
    private static let glow: [[Double]] = [
        [0.15, 0.80, 0.15, 0.05, 0.30, 0.05, 0.00, 0.00, 0.00],
        [0.70, 0.35, 0.00, 0.25, 0.15, 0.00, 0.00, 0.00, 0.00],
        [0.00, 0.35, 0.75, 0.00, 0.15, 0.30, 0.00, 0.00, 0.00],
        [0.00, 0.55, 0.00, 0.20, 0.55, 0.20, 0.00, 0.05, 0.00],
    ]
    private static let umber = Color(red: 0.31, green: 0.16, blue: 0.03)

    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 30, paused: !animated)) { context in
            MeshGradient(width: 3, height: 3, points: points(context.date.timeIntervalSinceReferenceDate), colors: colors)
        }
        .overlay {
            LinearGradient(colors: [.clear, Brand.ink.opacity(0.9)], startPoint: .init(x: 0.5, y: 0.45), endPoint: .bottom)
        }
        .ignoresSafeArea()
    }

    /// ink → umber → amber.
    private func ramp(_ g: Double) -> Color {
        g < 0.5 ? Brand.ink.mix(with: Self.umber, by: g * 2) : Self.umber.mix(with: Brand.amber, by: (g - 0.5) * 2)
    }

    private var colors: [Color] {
        let top = Self.glow.count - 1
        let lo = max(0, min(top, Int(progress.rounded(.down))))
        let hi = min(top, lo + 1)
        let f = Double(max(0, min(1, progress - CGFloat(lo))))
        return (0..<9).map { ramp(Self.glow[lo][$0] + (Self.glow[hi][$0] - Self.glow[lo][$0]) * f) }
    }

    /// A 3 × 3 grid whose inner points drift along their edges.
    private func points(_ t: TimeInterval) -> [SIMD2<Float>] {
        func d(_ speed: Double, _ phase: Double) -> Float { Float(0.035 * sin(t * speed + phase)) }
        return [
            [0, 0], [0.5 + d(0.5, 0), 0], [1, 0],
            [0, 0.5 + d(0.4, 1)], [0.5 + d(0.6, 2), 0.5 + d(0.5, 3)], [1, 0.5 + d(0.45, 4)],
            [0, 1], [0.5 + d(0.55, 5), 1], [1, 1],
        ]
    }
}

/// The amber capsule: ink words, a little press.
private struct AmberButton: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.headline)
            .foregroundStyle(Brand.ink)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 16)
            .background(Brand.amber, in: .capsule)
            .overlay(Capsule().strokeBorder(.white.opacity(0.35), lineWidth: 0.5))
            .shadow(color: Brand.amber.opacity(0.35), radius: 18, y: 8)
            .scaleEffect(configuration.isPressed ? 0.97 : 1)
            .animation(.snappy(duration: 0.2), value: configuration.isPressed)
    }
}
#endif
