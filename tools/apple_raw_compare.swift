// Compare Apple's Core Image RAW decoder (CIRAWFilter, RAW 9) with the LightCraft engine on the same raw files.
//
//   tools/apple_raw_compare.sh [options] FILES|FOLDERS…        (builds this file, then runs it)
//
// Per file it renders both at full size (sRGB, upright), then reports
//   - geometry: the two sizes (a different crop shows here first)
//   - tone/colour: mean L* difference, the exposure that best matches Apple to the engine, CIEDE2000 before and
//     after that exposure match (so a pure brightness difference is not read as a colour difference)
//   - noise and detail: measured in the same 16×16 blocks of both pictures and divided by the local contrast gain
//     between them, so Apple's stronger tone curve does not read as more noise
//   - time and which side could open the file at all
// Nothing here ships: it measures, it does not decide. Method and numbers: docs/apple-raw9.md.

import CoreImage
import Foundation
import ImageIO
import UniformTypeIdentifiers

// MARK: options

struct Options {
    var cli = ProcessInfo.processInfo.environment["LIGHTCRAFT_CLI"] ?? ""
    var out = "apple-raw-compare-out"
    var version = "newest"       // newest (the highest the file has), 9, 8, or default (what CIRAWFilter picks itself)
    var crop = 0                 // > 0: also write a centre crop of this many pixels, both sides, for looking at
    var lumNR: Float?            // Apple's luminance / colour noise reduction, 0...1 (its default varies per file)
    var colNR: Float?
    var nrOff = false            // Apple's noise reduction, luminance and colour, set to 0 (its default is on)
    var engineSets: [String] = [] // develop sliders for the engine render, CONTROL=VALUE
    var listModels = false
    var coverage = false         // no pixel metrics: only who can open each file (engine verdict, Apple's decoder and size)
    var files: [String] = []
}

func parse() -> Options {
    var o = Options()
    var args = CommandLine.arguments.dropFirst()
    while let a = args.popFirst() {
        switch a {
        case "--cli": o.cli = args.popFirst() ?? o.cli
        case "--out": o.out = args.popFirst() ?? o.out
        case "--version": o.version = args.popFirst() ?? o.version
        case "--crop": o.crop = Int(args.popFirst() ?? "") ?? 0
        case "--nr-off": o.nrOff = true
        case "--lum-nr": o.lumNR = Float(args.popFirst() ?? "")
        case "--col-nr": o.colNR = Float(args.popFirst() ?? "")
        case "--set": if let v = args.popFirst() { o.engineSets.append(v) }
        case "--list-models": o.listModels = true
        case "--coverage": o.coverage = true
        default: o.files.append(a)
        }
    }
    return o
}

// MARK: pixels

struct Pixels {
    var w: Int, h: Int
    var rgba: [UInt8]
}

func decodePNG(_ path: String) -> Pixels? {
    guard let src = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil),
          let img = CGImageSourceCreateImageAtIndex(src, 0, nil) else { return nil }
    return draw(img)
}

func draw(_ img: CGImage) -> Pixels? {
    let w = img.width, h = img.height
    guard w > 0, h > 0, let cs = CGColorSpace(name: CGColorSpace.sRGB) else { return nil }
    var buf = [UInt8](repeating: 0, count: w * h * 4)
    let ok = buf.withUnsafeMutableBytes { p -> Bool in
        guard let ctx = CGContext(data: p.baseAddress, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4, space: cs,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return false }
        ctx.draw(img, in: CGRect(x: 0, y: 0, width: w, height: h))
        return true
    }
    return ok ? Pixels(w: w, h: h, rgba: buf) : nil
}

func writePNG(_ p: Pixels, _ path: String) {
    guard let cs = CGColorSpace(name: CGColorSpace.sRGB) else { return }
    var data = p.rgba
    data.withUnsafeMutableBytes { raw in
        guard let ctx = CGContext(data: raw.baseAddress, width: p.w, height: p.h, bitsPerComponent: 8, bytesPerRow: p.w * 4, space: cs,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue),
              let img = ctx.makeImage(),
              let dest = CGImageDestinationCreateWithURL(URL(fileURLWithPath: path) as CFURL, UTType.png.identifier as CFString, 1, nil)
        else { return }
        CGImageDestinationAddImage(dest, img, nil)
        CGImageDestinationFinalize(dest)
    }
}

func centreCrop(_ p: Pixels, _ n: Int) -> Pixels {
    let cw = min(n, p.w), ch = min(n, p.h)
    let x0 = (p.w - cw) / 2, y0 = (p.h - ch) / 2
    var out = [UInt8](repeating: 0, count: cw * ch * 4)
    for y in 0..<ch {
        let s = ((y0 + y) * p.w + x0) * 4
        out.replaceSubrange(y * cw * 4..<(y + 1) * cw * 4, with: p.rgba[s..<s + cw * 4])
    }
    return Pixels(w: cw, h: ch, rgba: out)
}

// MARK: colour

@inline(__always) func lin(_ v: Float) -> Float { v <= 0.04045 ? v / 12.92 : powf((v + 0.055) / 1.055, 2.4) }

func xyz(_ r: Float, _ g: Float, _ b: Float) -> (Float, Float, Float) {
    (0.4124564 * r + 0.3575761 * g + 0.1804375 * b, 0.2126729 * r + 0.7151522 * g + 0.0721750 * b, 0.0193339 * r + 0.1191920 * g + 0.9503041 * b)
}

func lab(_ x: Float, _ y: Float, _ z: Float) -> (Float, Float, Float) {
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
    if Cp1 * Cp2 != 0 { hm = abs(hp1 - hp2) > 180 ? (hp1 + hp2 + (hp1 + hp2 < 360 ? 360 : -360)) / 2 : (hp1 + hp2) / 2 } else { hm = hp1 + hp2 }
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

// MARK: measurements

/// Average the picture into an nx × ny grid of linear-light cells (each side by its own size, so a crop that differs
/// by a few pixels still lines up to within a cell).
func grid(_ p: Pixels, _ nx: Int, _ ny: Int) -> [(Float, Float, Float)] {
    var acc = [(Float, Float, Float)](repeating: (0, 0, 0), count: nx * ny)
    var cnt = [Float](repeating: 0, count: nx * ny)
    var lutLin = [Float](repeating: 0, count: 256)
    for i in 0..<256 { lutLin[i] = lin(Float(i) / 255) }
    for y in 0..<p.h {
        let cy = min(ny - 1, y * ny / p.h)
        for x in 0..<p.w {
            let cx = min(nx - 1, x * nx / p.w), c = cy * nx + cx, i = (y * p.w + x) * 4
            acc[c].0 += lutLin[Int(p.rgba[i])]; acc[c].1 += lutLin[Int(p.rgba[i + 1])]; acc[c].2 += lutLin[Int(p.rgba[i + 2])]
            cnt[c] += 1
        }
    }
    for c in 0..<acc.count where cnt[c] > 0 { acc[c] = (acc[c].0 / cnt[c], acc[c].1 / cnt[c], acc[c].2 / cnt[c]) }
    return acc
}

func percentile(_ v: [Float], _ q: Float) -> Float {
    guard !v.isEmpty else { return .nan }
    let s = v.sorted()
    return s[min(s.count - 1, Int(q * Float(s.count)))]
}

struct Colour { var meanDL: Float, stops: Float, de: Float, deMatched: Float, deMatchedP95: Float, chroma: Float }

func colour(_ a: Pixels, _ e: Pixels) -> Colour {
    let nx = 96, ny = max(8, Int(Float(nx) * Float(e.h) / Float(e.w)))
    let ga = grid(a, nx, ny), ge = grid(e, nx, ny)
    // the exposure that best maps Apple's luminance onto the engine's (least squares in linear light)
    var num: Float = 0, den: Float = 0
    for i in 0..<ga.count { let ya = xyz(ga[i].0, ga[i].1, ga[i].2).1, ye = xyz(ge[i].0, ge[i].1, ge[i].2).1; num += ya * ye; den += ya * ya }
    let g = den > 0 ? num / den : 1
    var de: [Float] = [], dm: [Float] = [], dl: Float = 0, ca: Float = 0, ce: Float = 0
    for i in 0..<ga.count {
        let xa = xyz(ga[i].0, ga[i].1, ga[i].2), xe = xyz(ge[i].0, ge[i].1, ge[i].2)
        let la = lab(xa.0, xa.1, xa.2), le = lab(xe.0, xe.1, xe.2)
        let lm = lab(xa.0 * g, xa.1 * g, xa.2 * g)
        de.append(deltaE00(la, le)); dm.append(deltaE00(lm, le)); dl += la.0 - le.0
        ca += (la.1 * la.1 + la.2 * la.2).squareRoot(); ce += (le.1 * le.1 + le.2 * le.2).squareRoot()
    }
    let n = Float(ga.count)
    return Colour(meanDL: dl / n, stops: log2f(max(g, 1e-6)), de: de.reduce(0, +) / n, deMatched: dm.reduce(0, +) / n,
                  deMatchedP95: percentile(dm, 0.95), chroma: ce > 0 ? ca / ce : .nan)
}

/// Luma (sRGB-encoded, 0…1) of a picture cropped to the top-left w × h.
func luma(_ p: Pixels, _ w: Int, _ h: Int) -> [Float] {
    var l = [Float](repeating: 0, count: w * h)
    for y in 0..<h { for x in 0..<w { let i = (y * p.w + x) * 4
        l[y * w + x] = (0.2126 * Float(p.rgba[i]) + 0.7152 * Float(p.rgba[i + 1]) + 0.0722 * Float(p.rgba[i + 2])) / 255 } }
    return l
}

struct Block { var mean: Float, structure: Float, hf: Float }

/// 16×16 blocks: mean, structure (std of the sixteen 4×4 means: signal, hardly any noise) and hf (mean absolute
/// residual from the 4-neighbour average: noise on flat blocks, noise plus fine detail on edges).
func blocks(_ l: [Float], _ w: Int, _ h: Int) -> [Block] {
    var out: [Block] = []
    var by = 1
    while by + 16 < h - 1 {
        var bx = 1
        while bx + 16 < w - 1 {
            var sum: Float = 0, hf: Float = 0
            var m4 = [Float](repeating: 0, count: 16)
            for y in 0..<16 { for x in 0..<16 {
                let i = (by + y) * w + bx + x, v = l[i]
                sum += v
                hf += abs(v - 0.25 * (l[i - 1] + l[i + 1] + l[i - w] + l[i + w]))
                m4[(y / 4) * 4 + x / 4] += v / 16
            } }
            let mean = sum / 256, mm = m4.reduce(0, +) / 16
            let sd = (m4.reduce(0) { $0 + ($1 - mm) * ($1 - mm) } / 16).squareRoot()
            out.append(Block(mean: mean, structure: sd, hf: hf / 256))
            bx += 16
        }
        by += 16
    }
    return out
}

struct NoiseDetail { var noise: Float, detail: Float, gain: Float }

/// Apple over engine, per brightness band, with the local contrast gain divided out. 1.0 = the same; below 1 = less.
func noiseDetail(_ a: Pixels, _ e: Pixels) -> NoiseDetail {
    let w = min(a.w, e.w), h = min(a.h, e.h)
    guard w > 40, h > 40 else { return NoiseDetail(noise: .nan, detail: .nan, gain: .nan) }
    let ba = blocks(luma(a, w, h), w, h), be = blocks(luma(e, w, h), w, h)
    let n = min(ba.count, be.count)
    guard n > 30 else { return NoiseDetail(noise: .nan, detail: .nan, gain: .nan) }
    var noise: [Float] = [], detail: [Float] = [], gains: [Float] = []
    for band in 0..<3 {
        let lo = Float(band) / 3, hi = Float(band + 1) / 3
        let idx = (0..<n).filter { be[$0].mean >= lo && be[$0].mean < hi && be[$0].mean > 0.04 && be[$0].mean < 0.96 }
        guard idx.count > 30 else { continue }
        let byStructure = idx.sorted { be[$0].structure < be[$1].structure }
        let flat = byStructure.prefix(max(10, idx.count / 7)), edge = byStructure.suffix(max(10, idx.count / 10))
        let mid = byStructure.suffix(idx.count / 2)
        let gain = median(mid.compactMap { be[$0].structure > 1e-4 ? ba[$0].structure / be[$0].structure : nil })
        guard gain.isFinite, gain > 0.05 else { continue }
        gains.append(gain)
        noise.append(median(flat.compactMap { be[$0].hf > 1e-5 ? ba[$0].hf / be[$0].hf : nil }) / gain)
        detail.append(median(edge.compactMap { be[$0].hf > 1e-5 ? ba[$0].hf / be[$0].hf : nil }) / gain)
    }
    return NoiseDetail(noise: median(noise), detail: median(detail), gain: median(gains))
}

func median(_ v: [Float]) -> Float { v.isEmpty ? .nan : percentile(v, 0.5) }

// MARK: renderers

func shell(_ launch: String, _ args: [String]) -> (status: Int32, err: String, secs: Double) {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: launch)
    p.arguments = args
    let pipe = Pipe()
    p.standardError = pipe
    p.standardOutput = FileHandle.nullDevice
    let t0 = Date()
    do { try p.run() } catch { return (-1, "\(error)", 0) }
    let data = pipe.fileHandleForReading.readDataToEndOfFile()
    p.waitUntilExit()
    return (p.terminationStatus, String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines), Date().timeIntervalSince(t0))
}

/// The engine's own verdict on a file: "" = decodes, else why it is preview-only or fails to import.
func engineStatus(_ cli: String, _ file: String) -> String {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: cli)
    p.arguments = ["run", "--import", file, "photo.inspect", "id=1"]
    let pipe = Pipe()
    p.standardOutput = pipe
    p.standardError = FileHandle.nullDevice
    do { try p.run() } catch { return "cannot run lightcraft-cli" }
    let data = pipe.fileHandleForReading.readDataToEndOfFile()
    p.waitUntilExit()
    guard let line = String(decoding: data, as: UTF8.self).split(separator: "\n").first,
          let obj = try? JSONSerialization.jsonObject(with: Data(line.utf8)) as? [String: Any] else { return "import failed" }
    if obj["ok"] as? Bool != true { return "import failed: \(obj["error"] as? String ?? "?")" }
    let result = obj["result"] as? [String: Any]
    if let why = result?["preview_only"] as? String, !why.isEmpty { return "preview-only: \(why)" }
    return ""
}

func engineRender(_ cli: String, _ file: String, _ out: String, sets: [String]) -> (Pixels?, String, Double) {
    try? FileManager.default.removeItem(atPath: out)
    var args = ["render", file, "-o", out]
    for s in sets { args += ["--set", s] }
    let r = shell(cli, args)
    guard r.status == 0, let p = decodePNG(out) else { return (nil, r.err.split(separator: "\n").last.map(String.init) ?? "failed", r.secs) }
    return (p, "", r.secs)
}

func appleRender(_ file: String, version: String, nrOff: Bool, lumNR: Float?, colNR: Float?) -> (Pixels?, String, Double, String) {
    let t0 = Date()
    guard let raw = CIRAWFilter(imageURL: URL(fileURLWithPath: file)) else { return (nil, "CIRAWFilter cannot open it", 0, "") }
    let versions = raw.supportedDecoderVersions
    // a DNG's decoders are named "9.dng"; asking for "9" takes whichever of the two the file has
    let wanted: [CIRAWDecoderVersion] = version == "9" ? [.version9, .version9DNG] : version == "8" ? [.version8, .version8DNG] : []
    if version == "newest", let last = versions.last {
        raw.decoderVersion = last   // Apple's own default is not always the newest (the R6 opens with 8 unless asked for 9)
    } else if !wanted.isEmpty {
        guard let want = wanted.first(where: versions.contains) else {
            return (nil, "no decoder version \(version) (has \(versions.isEmpty ? "none" : versions.map(\.rawValue).joined(separator: ",")))", 0, "")
        }
        raw.decoderVersion = want
    }
    if nrOff {
        if raw.isLuminanceNoiseReductionSupported { raw.luminanceNoiseReductionAmount = 0 }
        if raw.isColorNoiseReductionSupported { raw.colorNoiseReductionAmount = 0 }
    }
    if let lumNR, raw.isLuminanceNoiseReductionSupported { raw.luminanceNoiseReductionAmount = lumNR }
    if let colNR, raw.isColorNoiseReductionSupported { raw.colorNoiseReductionAmount = colNR }
    FileHandle.standardError.write(Data("  \((file as NSString).lastPathComponent) decoder \(raw.decoderVersion.rawValue) lumNR \(raw.luminanceNoiseReductionAmount)(\(raw.isLuminanceNoiseReductionSupported)) colNR \(raw.colorNoiseReductionAmount)(\(raw.isColorNoiseReductionSupported)) sharp \(raw.sharpnessAmount) detail \(raw.detailAmount)\n".utf8))
    guard let image = raw.outputImage else { return (nil, "no output image", Date().timeIntervalSince(t0), raw.decoderVersion.rawValue) }
    let ctx = CIContext(options: [.cacheIntermediates: false])
    guard let cs = CGColorSpace(name: CGColorSpace.sRGB), let cg = ctx.createCGImage(image, from: image.extent, format: .RGBA8, colorSpace: cs),
          let px = draw(cg) else { return (nil, "render failed", Date().timeIntervalSince(t0), raw.decoderVersion.rawValue) }
    return (px, "", Date().timeIntervalSince(t0), raw.decoderVersion.rawValue)
}

// MARK: main

var o = parse()

if o.listModels {
    let all = CIRAWFilter.supportedCameraModels
    let v9 = CIRAWFilter.supportedCameraModels(with: .version9)
    print("supported models: \(all.count), with decoder 9: \(v9.count)")
    for m in v9.sorted() { print(m) }
    exit(0)
}

let rawExtensions: Set<String> = ["cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "raf", "orf", "ori", "rw2", "rwl", "pef", "srw", "x3f", "3fr", "fff", "iiq", "mos", "mef", "mrw", "dcr", "kdc", "erf", "gpr", "ari", "dng", "raw", "cap", "rwz", "dcs", "k25"]
var expanded: [String] = []
for path in o.files {
    var isDir: ObjCBool = false
    if FileManager.default.fileExists(atPath: path, isDirectory: &isDir), isDir.boolValue,
       let en = FileManager.default.enumerator(atPath: path) {
        for case let rel as String in en where rawExtensions.contains((rel as NSString).pathExtension.lowercased()) { expanded.append(path + "/" + rel) }
    } else { expanded.append(path) }
}
o.files = expanded.sorted()

guard !o.files.isEmpty else { FileHandle.standardError.write(Data("usage: apple_raw_compare [--cli PATH] [--out DIR] [--version newest|9|8|default] [--nr-off | --lum-nr X --col-nr Y] [--crop N] [--list-models] FILES…\n".utf8)); exit(2) }
var cli = o.cli
if cli.isEmpty {
    let env = ProcessInfo.processInfo.environment
    let cands = [env["CARGO_TARGET_DIR"].map { $0 + "/release/lightcraft-cli" }, "target/release/lightcraft-cli"].compactMap { $0 }
    cli = cands.first { FileManager.default.isExecutableFile(atPath: $0) } ?? ""
}
guard FileManager.default.isExecutableFile(atPath: cli) else { FileHandle.standardError.write(Data("no lightcraft-cli: pass --cli PATH or set LIGHTCRAFT_CLI\n".utf8)); exit(2) }
try? FileManager.default.createDirectory(atPath: o.out, withIntermediateDirectories: true)

setvbuf(stdout, nil, _IOLBF, 0)
func f(_ v: Float, _ d: Int = 2) -> String { v.isFinite ? String(format: "%.\(d)f", v) : "-" }
if o.coverage {
    print("file\tengine\tapple_decoder\tnative\tapple_ok\tapple_s\tnote")
    for file in o.files {
        let status = engineStatus(cli, file)
        let t0 = Date()
        var decoder = "-", native = "-", ok = "no", note = ""
        if let raw = CIRAWFilter(imageURL: URL(fileURLWithPath: file)) {
            let versions = raw.supportedDecoderVersions
            if let last = versions.last { raw.decoderVersion = last }
            decoder = raw.decoderVersion.rawValue
            native = "\(Int(raw.nativeSize.width))x\(Int(raw.nativeSize.height))"
            // a small render proves the decoder produces a picture, not just a size
            raw.scaleFactor = Float(min(1, 512 / max(1, max(raw.nativeSize.width, raw.nativeSize.height))))
            if let image = raw.outputImage, let cs = CGColorSpace(name: CGColorSpace.sRGB),
               let cg = CIContext().createCGImage(image, from: image.extent, format: .RGBA8, colorSpace: cs) {
                ok = "yes"
                if o.crop > 0, let px = draw(cg) {
                    let stem = ((file as NSString).lastPathComponent as NSString).deletingPathExtension
                    writePNG(px, "\(o.out)/\(stem).apple.small.png")
                }
            } else { note = "no picture" }
        } else { note = "CIRAWFilter cannot open it" }
        print([(file as NSString).lastPathComponent, status.isEmpty ? "decodes" : status, decoder, native, ok, f(Float(Date().timeIntervalSince(t0)), 2), note].joined(separator: "\t"))
    }
    exit(0)
}
print("file\tengine_s\tapple_s\tdecoder\tsize_engine\tsize_apple\tdL*\tstops\tdE00\tdE00_exp_matched\tdE00_p95\tchroma_ratio\tnoise_ratio\tdetail_ratio\tcontrast_gain\tnote")
for file in o.files {
    let name = (file as NSString).lastPathComponent
    let stem = (name as NSString).deletingPathExtension
    let status = engineStatus(cli, file)
    var (e, eErr, eSecs): (Pixels?, String, Double) = (nil, status, 0)
    if status.isEmpty { (e, eErr, eSecs) = engineRender(cli, file, "\(o.out)/\(stem).engine.png", sets: o.engineSets) }
    let (a, aErr, aSecs, dec) = appleRender(file, version: o.version, nrOff: o.nrOff, lumNR: o.lumNR, colNR: o.colNR)
    var cells = [name, f(Float(eSecs), 1), f(Float(aSecs), 1), dec.isEmpty ? "-" : dec, e.map { "\($0.w)x\($0.h)" } ?? "-", a.map { "\($0.w)x\($0.h)" } ?? "-"]
    var note = [String]()
    if !eErr.isEmpty { note.append("engine: \(eErr)") }
    if !aErr.isEmpty { note.append("apple: \(aErr)") }
    if let e, let a {
        let c = colour(a, e), nd = noiseDetail(a, e)
        cells += [f(c.meanDL, 1), f(c.stops), f(c.de), f(c.deMatched), f(c.deMatchedP95), f(c.chroma), f(nd.noise), f(nd.detail), f(nd.gain)]
        if abs(Float(a.w) / Float(a.h) - Float(e.w) / Float(e.h)) > 0.005 { note.append("different aspect") }
        else if abs(a.w - e.w) > 8 { note.append("different size") }
        if o.crop > 0 { writePNG(centreCrop(e, o.crop), "\(o.out)/\(stem).engine.crop.png"); writePNG(centreCrop(a, o.crop), "\(o.out)/\(stem).apple.crop.png") }
    } else {
        if o.crop > 0, let a { writePNG(centreCrop(a, o.crop), "\(o.out)/\(stem).apple.crop.png") }
        cells += Array(repeating: "-", count: 9)
    }
    cells.append(note.joined(separator: "; "))
    print(cells.joined(separator: "\t"))
}
