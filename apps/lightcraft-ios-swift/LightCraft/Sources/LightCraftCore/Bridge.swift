import CLightCraftBridge
import Foundation

/// The C ABI (`lightcraft_bridge.h`), the only place that touches it. Every buffer the library
/// hands out is copied into Swift memory and released here, exactly once; nothing else sees a
/// pointer.
enum Bridge {
    /// The ABI the library speaks; `LC_ABI_VERSION` is the one this code was written for.
    static var abiVersion: UInt32 { lc_abi_version() }
    static var expectedAbiVersion: UInt32 { LC_ABI_VERSION }

    static func open(config: Data) -> Result<Data, ErrorBody> {
        take(config.withUnsafeBytes { raw in lc_open(raw.bindMemory(to: UInt8.self).baseAddress, raw.count) })
    }

    static func close(_ engine: UInt64) {
        lc_close(engine)
    }

    /// Queue a request; nil when it was taken, else why not.
    static func submit(_ engine: UInt64, _ request: Data) -> ErrorBody? {
        let status = request.withUnsafeBytes { raw in lc_submit(engine, raw.bindMemory(to: UInt8.self).baseAddress, raw.count) }
        switch status {
        case LC_OK: return nil
        case LC_BAD_HANDLE: return ErrorBody(code: "badHandle", message: "the engine is closed")
        case LC_CLOSED: return ErrorBody(code: "closed", message: "the engine has stopped")
        case LC_PANIC: return ErrorBody(code: "panic", message: "LightCraft hit an internal error")
        default: return ErrorBody(code: "badRequest", message: "the request was refused")
        }
    }

    static func call(_ engine: UInt64, _ request: Data, timeoutMs: UInt32) -> Result<Data, ErrorBody> {
        take(request.withUnsafeBytes { raw in lc_call(engine, raw.bindMemory(to: UInt8.self).baseAddress, raw.count, timeoutMs) })
    }

    /// The events since the last poll (a JSON array), waiting up to `timeoutMs` for one.
    static func poll(_ engine: UInt64, timeoutMs: UInt32) -> Result<Data, ErrorBody> {
        take(lc_poll(engine, timeoutMs))
    }

    static func wake(_ engine: UInt64) {
        lc_wake(engine)
    }

    /// The pixels of an image event's token, copied; the token is released either way. With `hdr`, its
    /// float rendition too (a lease of its own, released either way).
    static func image(token: UInt64, hdr: HdrRef? = nil) -> PixelImage? {
        let float = hdr.flatMap { hdrImage(token: $0.token, peak: $0.peak) }
        let image = lc_image(token)
        defer { lc_release(token) }
        guard image.status == LC_OK, let pixels = image.pixels, image.len > 0 else { return nil }
        return PixelImage(width: Int(image.width), height: Int(image.height), stride: Int(image.stride), rgba: Data(bytes: pixels, count: image.len), hdr: float)
    }

    /// The RGBA16F pixels of an image event's `hdr` token, copied; released either way.
    private static func hdrImage(token: UInt64, peak: Double) -> HdrImage? {
        let image = lc_image(token)
        defer { lc_release(token) }
        guard image.status == LC_OK, let pixels = image.pixels, image.len > 0, Int(image.stride) >= Int(image.width) * 8 else { return nil }
        return HdrImage(width: Int(image.width), height: Int(image.height), stride: Int(image.stride), rgba16f: Data(bytes: pixels, count: image.len), peak: peak)
    }

    /// Copy a buffer's bytes and release it: its JSON, or the error it holds.
    private static func take(_ buf: LcBuf) -> Result<Data, ErrorBody> {
        defer {
            if buf.token != 0 { lc_release(buf.token) }
        }
        let data: Data
        if let ptr = buf.ptr, buf.len > 0 {
            data = Data(bytes: ptr, count: buf.len)
        } else {
            data = Data()
        }
        if buf.status == LC_OK {
            return .success(data)
        }
        if let error = try? JSONDecoder().decode(ErrorBody.self, from: data) {
            return .failure(error)
        }
        return .failure(ErrorBody(code: "status\(buf.status)", message: "the engine refused (status \(buf.status))"))
    }
}
