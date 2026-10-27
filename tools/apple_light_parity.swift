// How close can Core Image's own filters get to the engine's Light controls (exposure, contrast, highlights, shadows,
// whites, blacks)?
//
//   tools/apple_light_parity.sh --cli target/release/lightcraft-cli [--out DIR] [--size 1600] [--raw FILE…] IMAGES…
//
// Two arms. Display-referred pictures (JPEG / PNG): both sides start from the same pixels, so the error is a
// per-pixel CIEDE2000 against the engine's render. Raws (--raw): the two sides start from different decodes (the engine's
// look, Apple's RAW 9), so only the response is compared: the mean L* change the control makes in the shadows, mids and
// highlights of each side's own picture.
//
// Three tiers per control and value, so the result separates "what Apple ships" from "what a 1-D curve can express":
//   builtin  one Core Image filter chosen the obvious way (CIExposureAdjust, CIColorControls, CIHighlightShadowAdjust,
//            CIToneCurve), with its strength fitted per value to the engine's render (the best case for that filter)
//   curve    the engine's own response on a grey ramp, loaded as a 1-D curve (CIColorCurves): the ceiling for any filter
//            that treats every pixel the same
//   none     the picture left alone: the size of the edit itself, to read the other errors against
// Nothing here ships: it measures, it does not decide. Method and numbers: docs/apple-light-parity.md.

import CoreImage
import CoreImage.CIFilterBuiltins
import Foundation
import ImageIO
import UniformTypeIdentifiers

// MARK: options

struct Options {
    var cli = ProcessInfo.processInfo.environment["LIGHTCRAFT_CLI"] ?? ""
    var out = "apple-light-parity-out"
    var size = 1600
    var linear = false          // Core Image's own default: filters work in linear light (else encoded sRGB)
    var raws: [String] = []
    var files: [String] = []
}

func parse() -> Options {
    var o = Options()
    var args = CommandLine.arguments.dropFirst()
    while let a = args.popFirst() {
        switch a {
        case "--cli": o.cli = args.popFirst() ?? o.cli
        case "--out": o.out = args.popFirst() ?? o.out
        case "--linear": o.linear = true
        case "--size": o.size = Int(args.popFirst() ?? "") ?? o.size
        case "--raw": if let f = args.popFirst() { o.raws.append(f) }
        default: o.files.append(a)
        }
    }
    return o
}

// MARK: pixels and colour (as in apple_raw_compare.swift)

struct Pixels { var w: Int, h: Int, rgba: [UInt8] }

let srgb = CGColorSpace(name: CGColorSpace.sRGB)!

func draw(_ img: CGImage) -> Pixels? {
    let w = img.width, h = img.height
    guard w > 0, h > 0 else { return nil }
    var buf = [UInt8](repeating: 0, count: w * h * 4)
    let ok = buf.withUnsafeMutableBytes { p -> Bool in
        guard let ctx = CGContext(data: p.baseAddress, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4, space: srgb,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return false }
        ctx.draw(img, in: CGRect(x: 0, y: 0, width: w, height: h))
        return true
    }
    return ok ? Pixels(w: w, h: h, rgba: buf) : nil
}

func decode(_ path: String) -> Pixels? {
    guard let src = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil),
          let img = CGImageSourceCreateImageAtIndex(src, 0, nil) else { return nil }
    return draw(img)
}

@inline(__always) func lin(_ v: Float) -> Float { v <= 0.04045 ? v / 12.92 : powf((v + 0.055) / 1.055, 2.4) }

func lab(_ r8: UInt8, _ g8: UInt8, _ b8: UInt8) -> (Float, Float, Float) {
    let r = lin(Float(r8) / 255), g = lin(Float(g8) / 255), b = lin(Float(b8) / 255)
    let x = 0.4124564 * r + 0.3575761 * g + 0.1804375 * b, y = 0.2126729 * r + 0.7151522 * g + 0.0721750 * b
    let z = 0.0193339 * r + 0.1191920 * g + 0.9503041 * b
    func f(_ t: Float) -> Float { t > 0.008856 ? cbrtf(t) : 7.787 * t + 16 / 116 }
    let fx = f(x / 0.95047), fy = f(y), fz = f(z / 1.08883)
    return (116 * fy - 16, 500 * (fx - fy), 200 * (fy - fz))
}

func deltaE00(_ a: (Float, Float, Float), _ b: (Float, Float, Float)) -> Float {
    let (L1, a1, b1) = (Double(a.0), Double(a.1), Double(a.2)), (L2, a2, b2) = (Double(b.0), Double(b.1), Double(b.2))
    let C1 = (a1 * a1 + b1 * b1).squareRoot(), C2 = (a2 * a2 + b2 * b2).squareRoot()
    let Cm = (C1 + C2) / 2, Cm7 = pow(Cm, 7)
    let G = 0.5 * (1 - (Cm7 / (Cm7 + pow(25, 7))).squareRoot())
    let ap1 = (1 + G) * a1, ap2 = (1 + G) * a2
    let Cp1 = (ap1 * ap1 + b1 * b1).squareRoot(), Cp2 = (ap2 * ap2 + b2 * b2).squareRoot()
    func hue(_ b: Double, _ a: Double) -> Double { if a == 0 && b == 0 { return 0 }; let h = atan2(b, a) * 180 / .pi; return h < 0 ? h + 360 : h }
    let hp1 = hue(b1, ap1), hp2 = hue(b2, ap2)
    let dL = L2 - L1, dC = Cp2 - Cp1
    var dh = 0.0
    if Cp1 * Cp2 != 0 { dh = hp2 - hp1; if dh > 180 { dh -= 360 } else if dh < -180 { dh += 360 } }
    let dH = 2 * (Cp1 * Cp2).squareRoot() * sin(dh * .pi / 360)
    let Lm = (L1 + L2) / 2, Cpm = (Cp1 + Cp2) / 2
    var hm = hp1 + hp2
    if Cp1 * Cp2 != 0 { hm = abs(hp1 - hp2) > 180 ? (hp1 + hp2 + (hp1 + hp2 < 360 ? 360 : -360)) / 2 : (hp1 + hp2) / 2 }
    let T = 1 - 0.17 * cos((hm - 30) * .pi / 180) + 0.24 * cos(2 * hm * .pi / 180) + 0.32 * cos((3 * hm + 6) * .pi / 180) - 0.20 * cos((4 * hm - 63) * .pi / 180)
    let dTheta = 30 * exp(-pow((hm - 275) / 25, 2))
    let Cpm7 = pow(Cpm, 7)
    let Rc = 2 * (Cpm7 / (Cpm7 + pow(25, 7))).squareRoot()
    let Sl = 1 + 0.015 * pow(Lm - 50, 2) / (20 + pow(Lm - 50, 2)).squareRoot()
    let Sc = 1 + 0.045 * Cpm, Sh = 1 + 0.015 * Cpm * T
    let Rt = -sin(2 * dTheta * .pi / 180) * Rc
    let e = pow(dL / Sl, 2) + pow(dC / Sc, 2) + pow(dH / Sh, 2) + Rt * (dC / Sc) * (dH / Sh)
    return Float(max(0, e).squareRoot())
}

func percentile(_ v: [Float], _ q: Float) -> Float {
    guard !v.isEmpty else { return .nan }
    let s = v.sorted()
    return s[min(s.count - 1, Int(q * Float(s.count)))]
}
func median(_ v: [Float]) -> Float { percentile(v, 0.5) }

/// Per-pixel CIEDE2000 between two pictures of one size, over about 40 000 evenly spread pixels.
func deltaE(_ a: Pixels, _ b: Pixels) -> (mean: Float, p95: Float) {
    guard a.w == b.w, a.h == b.h else { return (.nan, .nan) }
    let step = max(1, Int((Double(a.w * a.h) / 40_000).squareRoot()))
    var d: [Float] = []
    for y in stride(from: 0, to: a.h, by: step) {
        for x in stride(from: 0, to: a.w, by: step) {
            let i = (y * a.w + x) * 4
            d.append(deltaE00(lab(a.rgba[i], a.rgba[i + 1], a.rgba[i + 2]), lab(b.rgba[i], b.rgba[i + 1], b.rgba[i + 2])))
        }
    }
    return (d.reduce(0, +) / Float(max(1, d.count)), percentile(d, 0.95))
}

/// Mean L* change an edit makes in the base picture's shadows (L* < 30), mids and highlights (L* > 70), pixel by pixel.
func zones(base: Pixels, edited: Pixels) -> [Float] {
    guard base.w == edited.w, base.h == edited.h else { return [.nan, .nan, .nan] }
    var sum: [Float] = [0, 0, 0], n: [Float] = [0, 0, 0]
    let step = max(1, Int((Double(base.w * base.h) / 60_000).squareRoot()))
    for y in stride(from: 0, to: base.h, by: step) {
        for x in stride(from: 0, to: base.w, by: step) {
            let i = (y * base.w + x) * 4
            let lb = lab(base.rgba[i], base.rgba[i + 1], base.rgba[i + 2]).0, le = lab(edited.rgba[i], edited.rgba[i + 1], edited.rgba[i + 2]).0
            let z = lb < 30 ? 0 : (lb > 70 ? 2 : 1)
            sum[z] += le - lb; n[z] += 1
        }
    }
    return (0..<3).map { n[$0] > 0 ? sum[$0] / n[$0] : .nan }
}

// MARK: Core Image

let ciContext = CIContext(options: [.workingColorSpace: CommandLine.arguments.contains("--linear") ? CGColorSpace(name: CGColorSpace.linearSRGB)! : srgb, .outputColorSpace: srgb])

func scaled(_ img: CIImage, longEdge: Int) -> CIImage {
    let e = img.extent
    let s = CGFloat(longEdge) / max(e.width, e.height)
    guard s < 1 else { return img }
    let f = CIFilter.lanczosScaleTransform()
    f.inputImage = img; f.scale = Float(s); f.aspectRatio = 1
    return (f.outputImage ?? img).cropped(to: CGRect(x: 0, y: 0, width: (e.width * s).rounded(), height: (e.height * s).rounded()))
}

func render(_ img: CIImage) -> Pixels? {
    let w = Int(img.extent.width), h = Int(img.extent.height)
    var buf = [UInt8](repeating: 0, count: w * h * 4)
    buf.withUnsafeMutableBytes { ciContext.render(img, toBitmap: $0.baseAddress!, rowBytes: w * 4, bounds: img.extent, format: .RGBA8, colorSpace: srgb) }
    return Pixels(w: w, h: h, rgba: buf)
}

func writePNG(_ p: Pixels, _ path: String) {
    var data = p.rgba
    data.withUnsafeMutableBytes { raw in
        guard let ctx = CGContext(data: raw.baseAddress, width: p.w, height: p.h, bitsPerComponent: 8, bytesPerRow: p.w * 4, space: srgb,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue),
              let img = ctx.makeImage(),
              let dest = CGImageDestinationCreateWithURL(URL(fileURLWithPath: path) as CFURL, UTType.png.identifier as CFString, 1, nil)
        else { return }
        CGImageDestinationAddImage(dest, img, nil)
        CGImageDestinationFinalize(dest)
    }
}

/// One Core Image filter for a control, the obvious way. `k` scales the slider into the filter's own range and `radius`
/// is CIHighlightShadowAdjust's; both are fitted by the caller.
func builtin(_ control: String, _ v: Double, k: Double, radius: Double, _ img: CIImage) -> CIImage {
    let a = v / 100 * k
    switch control {
    case "exposure":
        let f = CIFilter.exposureAdjust(); f.inputImage = img; f.ev = Float(v * k); return f.outputImage ?? img
    case "contrast":
        let f = CIFilter.colorControls(); f.inputImage = img; f.contrast = Float(1 + a); return f.outputImage ?? img
    case "highlights":
        let f = CIFilter.highlightShadowAdjust(); f.inputImage = img
        f.highlightAmount = Float(min(1, max(0, 1 + a))); f.shadowAmount = 0; f.radius = Float(radius); return f.outputImage ?? img
    case "shadows":
        let f = CIFilter.highlightShadowAdjust(); f.inputImage = img
        f.shadowAmount = Float(min(1, max(-1, a))); f.highlightAmount = 1; f.radius = Float(radius); return f.outputImage ?? img
    case "whites", "blacks":
        // five points on x = 0, ¼, ½, ¾, 1; blacks move the low end, whites the high end
        var y: [Double] = [0, 0.25, 0.5, 0.75, 1]
        if control == "blacks" { y[0] = max(0, a); y[1] = 0.25 + a * 0.5; y[2] = 0.5 + a * 0.15 }
        else { y[4] = min(1, max(0, 1 + min(0, a))); y[3] = 0.75 + a * 0.5; y[2] = 0.5 + a * 0.15 }
        for i in 1..<5 { y[i] = min(1, max(y[i - 1], y[i])) }
        let f = CIFilter.toneCurve(); f.inputImage = img
        f.point0 = CGPoint(x: 0, y: y[0]); f.point1 = CGPoint(x: 0.25, y: y[1]); f.point2 = CGPoint(x: 0.5, y: y[2])
        f.point3 = CGPoint(x: 0.75, y: y[3]); f.point4 = CGPoint(x: 1, y: y[4])
        return f.outputImage ?? img
    default: return img
    }
}

/// A 1-D curve (256 samples, encoded sRGB in and out) as a filter.
func curve(_ lut: [Float], _ img: CIImage) -> CIImage {
    var data = [Float](); data.reserveCapacity(lut.count * 3)
    for v in lut { data += [v, v, v] }
    let f = CIFilter.colorCurves(); f.inputImage = img
    f.curvesData = data.withUnsafeBytes { Data($0) }; f.curvesDomain = CIVector(x: 0, y: 1); f.colorSpace = srgb
    return f.outputImage ?? img
}

// MARK: engine

func shell(_ launch: String, _ args: [String]) -> (status: Int32, secs: Double) {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: launch)
    p.arguments = args
    p.standardError = FileHandle.nullDevice; p.standardOutput = FileHandle.nullDevice
    let t0 = Date()
    do { try p.run() } catch { return (-1, 0) }
    p.waitUntilExit()
    return (p.terminationStatus, Date().timeIntervalSince(t0))
}

func engine(_ cli: String, _ file: String, _ out: String, _ sets: [String]) -> Pixels? {
    try? FileManager.default.removeItem(atPath: out)
    var args = ["render", file, "-o", out]
    for s in sets { args += ["--set", s] }
    return shell(cli, args).status == 0 ? decode(out) : nil
}

// MARK: study

let opt = parse()
guard !opt.cli.isEmpty, FileManager.default.isExecutableFile(atPath: opt.cli) else {
    FileHandle.standardError.write(Data("need --cli PATH to lightcraft-cli (or LIGHTCRAFT_CLI)\n".utf8)); exit(2)
}
guard !opt.files.isEmpty || !opt.raws.isEmpty else {
    FileHandle.standardError.write(Data("need pictures (display-referred) and/or --raw FILE\n".utf8)); exit(2)
}
try? FileManager.default.createDirectory(atPath: opt.out, withIntermediateDirectories: true)

let values: [(String, [Double])] = [
    ("exposure", [-1, 1]), ("contrast", [-50, 50, 100]), ("highlights", [-50, -100, 50]),
    ("shadows", [-50, 50, 100]), ("whites", [-50, 50]), ("blacks", [-50, 50]),
]
let ks: [String: [Double]] = [
    "exposure": [0.25, 0.35, 0.5, 0.75, 1, 1.25, 1.5], "contrast": [0.05, 0.1, 0.15, 0.2, 0.3, 0.5, 0.7, 1.0],
    "highlights": [0.25, 0.5, 1, 2, 4], "shadows": [0.25, 0.5, 1, 2, 4],
    "whites": [0.03, 0.05, 0.1, 0.2, 0.3, 0.5, 0.8], "blacks": [0.03, 0.05, 0.1, 0.2, 0.3, 0.5, 0.8],
]
/// Whether a fitted strength sits at the end of its candidates (the real optimum may lie beyond).
func atEdge(_ control: String, _ k: Double) -> Bool { k == ks[control]?.first || k == ks[control]?.last }

/// The engine's response to a control on a grey ramp, as a 256-sample curve.
func rampCurve(_ control: String, _ v: Double) -> [Float]? {
    let ramp = "\(opt.out)/ramp.png"
    if !FileManager.default.fileExists(atPath: ramp) {
        var p = Pixels(w: 256, h: 8, rgba: [UInt8](repeating: 255, count: 256 * 8 * 4))
        for y in 0..<8 { for x in 0..<256 { let i = (y * 256 + x) * 4; p.rgba[i] = UInt8(x); p.rgba[i + 1] = UInt8(x); p.rgba[i + 2] = UInt8(x) } }
        writePNG(p, ramp)
    }
    guard let r = engine(opt.cli, ramp, "\(opt.out)/ramp-out.png", ["light.\(control)=\(v)"]), r.w == 256 else { return nil }
    let row = 4 * r.w
    return (0..<256).map { Float(r.rgba[row * 4 + $0 * 4]) / 255 }
}

func name(_ path: String) -> String { URL(fileURLWithPath: path).deletingPathExtension().lastPathComponent }
func f(_ v: Float, _ d: Int = 2) -> String { v.isFinite ? String(format: "%.\(d)f", v) : "-" }

var bestK: [String: (Double, Double)] = [:]   // "control|value" → (k, radius) chosen on the display-referred pictures
var tierErrors: [String: [String: [Float]]] = [:]   // "control|value" → tier → per-picture mean dE00

print("arm\tpicture\tcontrol\tvalue\ttier\tparam\tdE00_mean\tdE00_p95\tnote")
for file in opt.files {
    guard let ci = CIImage(contentsOf: URL(fileURLWithPath: file)) else { continue }
    let base = scaled(ci, longEdge: opt.size)
    guard let basePx = render(base) else { continue }
    let basePath = "\(opt.out)/\(name(file)).base.png"
    writePNG(basePx, basePath)
    for (control, vs) in values {
        for v in vs {
            let key = "\(control)|\(v)"
            guard let eng = engine(opt.cli, basePath, "\(opt.out)/\(name(file)).\(control)\(Int(v * 100)).png", ["light.\(control)=\(v)"]) else {
                print("display\t\(name(file))\t\(control)\t\(v)\tengine\t-\t-\t-\tengine render failed"); continue
            }
            let none = deltaE(basePx, eng)
            print("display\t\(name(file))\t\(control)\t\(v)\tnone\t-\t\(f(none.mean))\t\(f(none.p95))\tthe size of the edit")
            tierErrors[key, default: [:]]["none", default: []].append(none.mean)
            // builtin: fitted strength (and radius, for highlights and shadows), best of the candidates
            var best: (Float, Float, Double, Double)? = nil
            for k in ks[control] ?? [1] {
                for radius in (control == "highlights" || control == "shadows") ? [0.0, 10, 40] : [0] {
                    guard let out = render(builtin(control, v, k: k, radius: radius, base)) else { continue }
                    let e = deltaE(out, eng)
                    if best == nil || e.mean < best!.0 { best = (e.mean, e.p95, k, radius) }
                }
            }
            if let b = best {
                print("display\t\(name(file))\t\(control)\t\(v)\tbuiltin\tk=\(b.2) r=\(b.3)\t\(f(b.0))\t\(f(b.1))\t\(atEdge(control, b.2) ? "best strength is at the end of the candidates" : "best strength found")")
                tierErrors[key, default: [:]]["builtin", default: []].append(b.0)
                bestK[key] = (b.2, b.3)
            }
            if let lut = rampCurve(control, v), let out = render(curve(lut, base)) {
                let e = deltaE(out, eng)
                print("display\t\(name(file))\t\(control)\t\(v)\tcurve\t-\t\(f(e.mean))\t\(f(e.p95))\tthe engine's ramp response as a 1-D curve")
                tierErrors[key, default: [:]]["curve", default: []].append(e.mean)
            }
        }
    }
}

// raws: response by zone (shadows / mids / highlights), each side against its own base
for file in opt.raws {
    guard let rf = CIRAWFilter(imageURL: URL(fileURLWithPath: file)), let apple = rf.outputImage else {
        print("raw\t\(name(file))\t-\t-\t-\t-\t-\t-\tApple cannot open it"); continue
    }
    let abase = scaled(apple, longEdge: opt.size)
    guard let aPx = render(abase), let e0 = engine(opt.cli, file, "\(opt.out)/\(name(file)).engine-base.png", []) else { continue }
    let ePx = e0
    for (control, vs) in values {
        for v in vs {
            guard let eEdit = engine(opt.cli, file, "\(opt.out)/\(name(file)).\(control)\(Int(v * 100)).png", ["light.\(control)=\(v)"]) else { continue }
            // the strength that brings Core Image's zone response nearest the engine's (these controls barely move a
            // display-referred picture, so the strength fitted on those says nothing about a raw)
            let ez = zones(base: ePx, edited: eEdit)
            var best: (Float, Double, Double, [Float])? = nil
            for k in ks[control] ?? [1] {
                for radius in (control == "highlights" || control == "shadows") ? [0.0, 10, 40] : [0] {
                    guard let aEdit = render(builtin(control, v, k: k, radius: radius, abase)) else { continue }
                    let az = zones(base: aPx, edited: aEdit)
                    let miss = zip(ez, az).map { abs($0 - $1) }.reduce(0, +)
                    if best == nil || miss < best!.0 { best = (miss, k, radius, az) }
                }
            }
            guard let b = best else { continue }
            print("raw\t\(name(file))\t\(control)\t\(v)\tzones dL* (shadows/mids/highlights)\tk=\(b.1) r=\(b.2)\tengine \(ez.map { f($0, 1) }.joined(separator: "/"))\tcore image \(b.3.map { f($0, 1) }.joined(separator: "/"))\tmiss \(f(b.0, 1))\(atEdge(control, b.1) ? ", strength at the end of the candidates" : "")")
        }
    }
}

// summary: median over the pictures
print("\nsummary (median over \(opt.files.count) display-referred picture(s); mean dE00 against the engine, lower is closer)")
print("control\tvalue\tnone\tbuiltin\tcurve")
for (control, vs) in values {
    for v in vs {
        let t = tierErrors["\(control)|\(v)"] ?? [:]
        print("\(control)\t\(v)\t\(f(median(t["none"] ?? [])))\t\(f(median(t["builtin"] ?? [])))\t\(f(median(t["curve"] ?? [])))")
    }
}

// speed: the chain on a 2.5 MP picture, a warm render to a bitmap (what a draft costs), median of 20
if let first = opt.files.first, let ci = CIImage(contentsOf: URL(fileURLWithPath: first)) {
    let preview = scaled(ci, longEdge: 1920)
    func ms(_ make: (Double) -> CIImage) -> Float {
        var t: [Float] = []
        for i in 0..<23 {
            // (a different strength every time, so no render can be answered from a cache)
            let t0 = Date(); _ = render(make(Double(i % 7 + 1) / 7)); if i >= 3 { t.append(Float(Date().timeIntervalSince(t0) * 1000)) }
        }
        return median(t)
    }
    print("\nspeed on \(Int(preview.extent.width))x\(Int(preview.extent.height)), ms per warm render (includes the read-back)")
    print("chain\tms")
    print("passthrough\t\(f(ms { _ in preview }, 1))")
    let lut = rampCurve("contrast", 50) ?? (0..<256).map { Float($0) / 255 }
    func four(_ t: Double) -> CIImage {
        builtin("blacks", 50 * t, k: 0.3, radius: 0, builtin("whites", 50 * t, k: 0.3, radius: 0, builtin("contrast", 50 * t, k: 0.3, radius: 0, builtin("exposure", t, k: 1, radius: 0, preview))))
    }
    print("exposure + contrast + whites + blacks (builtin)\t\(f(ms { four($0) }, 1))")
    print("+ highlights + shadows (CIHighlightShadowAdjust)\t\(f(ms { builtin("shadows", 50 * $0, k: 1, radius: 10, builtin("highlights", -50 * $0, k: 1, radius: 10, four($0))) }, 1))")
    print("1-D curve (CIColorCurves)\t\(f(ms { t in curve(lut.map { $0 * Float(0.9 + 0.1 * t) }, preview) }, 1))")
}
var usage = rusage(); getrusage(RUSAGE_SELF, &usage)
print("\npeak resident memory of this run: \(usage.ru_maxrss / 1_048_576) MB")
