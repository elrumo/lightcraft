#if os(iOS)
import CoreGraphics
import LightCraftCore
import Observation

/// The pictures the engine sent, by slot (`thumb:42`, `thumbQuick:42`, `main`, `preview`, `before`).
@Observable
final class ImageStore {
    struct Picture {
        let image: CGImage
        /// The same render as float pixels, for an HDR edit on a screen with headroom: drawn instead of
        /// `image` with `allowedDynamicRange(.high)` (`AppModel.hdrDisplay`).
        let hdr: CGImage?
        let photo: UInt64
        let key: UInt64
        let isFinal: Bool
        let draft: Bool
        let ms: Double
        let histogram: Histogram?
        /// A zoom window's place in the zoomed frame (`region`).
        let window: ZoomWindow?
    }

    private(set) var pictures: [String: Picture] = [:]
    var failed: [String: String] = [:]
    /// Thumbnails kept in memory beyond those on screen.
    private let maxThumbs = 600

    func put(_ info: ImageEvent, _ image: CGImage, hdr: CGImage? = nil) {
        pictures[info.slot] = Picture(image: image, hdr: hdr, photo: info.photo, key: info.key, isFinal: info.isFinal, draft: info.draft, ms: info.ms, histogram: info.histogram,
                                      window: info.window)
        failed[info.slot] = nil
        // the real thumbnail replaces its stand-in
        if info.slot.hasPrefix("thumb:") {
            pictures["thumbQuick:\(info.photo)"] = nil
        }
    }

    /// A grid cell's picture: the thumbnail, else its stand-in.
    func thumb(_ photo: UInt64) -> Picture? {
        pictures["thumb:\(photo)"] ?? pictures["thumbQuick:\(photo)"]
    }

    /// The loupe's picture of `photo`: its render, else the stand-in, else the thumbnail.
    func view(for photo: UInt64) -> Picture? {
        if let main = pictures["main"], main.photo == photo { return main }
        if let preview = pictures["preview"], preview.photo == photo { return preview }
        return thumb(photo)
    }

    /// The zoomed loupe's window of `photo`, drawn over its render.
    func region(for photo: UInt64) -> Picture? {
        if let r = pictures["region"], r.photo == photo, r.window != nil { return r }
        return nil
    }

    /// An edit landed: the window shows the photo as it was, until the next one comes.
    func dropRegion() {
        pictures["region"] = nil
    }

    /// The Before of `photo` (held, or the Before / After view).
    func before(for photo: UInt64) -> Picture? {
        if let b = pictures["before"], b.photo == photo { return b }
        return nil
    }

    /// Let thumbnails go, beyond `maxThumbs`, that aren't on screen.
    func trimThumbs(keeping visible: Set<UInt64>) {
        let thumbs = pictures.keys.filter { $0.hasPrefix("thumb") }
        guard thumbs.count > maxThumbs else { return }
        for slot in thumbs where !visible.contains(pictures[slot]?.photo ?? 0) {
            pictures[slot] = nil
        }
    }

    /// Another library: photo ids mean something else now.
    func forgetAll() {
        pictures.removeAll()
        failed.removeAll()
    }
}
#endif
