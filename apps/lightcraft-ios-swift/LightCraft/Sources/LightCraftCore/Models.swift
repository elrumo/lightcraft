import Foundation

// Engine answers the app decodes. Their shapes are the engine's (crates/engine): photo summaries
// (`cmd/query.rs` photo_summary), command info (`cmd/mod.rs` CommandInfo), control specs
// (`crates/develop/src/controls.rs` ControlSpec + its current value).

/// A photo as `catalog.query` lists it.
public struct PhotoSummary: Codable, Sendable, Identifiable, Hashable {
    public var id: UInt64
    public var fileName: String
    public var format: String?
    /// `image`, `raw` or `video`.
    public var kind: String?
    public var width: Int
    public var height: Int
    public var captured: String?
    public var rating: Int?
    /// `none`, `pick` or `reject`.
    public var flag: String?
    /// `red`, `yellow`, `green`, `blue`, `purple` or nil.
    public var label: String?
    public var edited: Bool?
    public var title: String?
    public var camera: String?
    public var keywords: [String]?
    public var deleted: Bool?
    /// A virtual copy's name.
    public var copyName: String?
    /// Why only a preview of it is here (a synced photo whose original is on the server).
    public var previewOnly: String?

    /// Width over height (1 when unknown).
    public var aspect: Double {
        width > 0 && height > 0 ? Double(width) / Double(height) : 1
    }

    public var isPick: Bool { flag == "pick" }
    public var isReject: Bool { flag == "reject" }
}

/// A page of `catalog.query`.
public struct PhotoPage: Codable, Sendable {
    public var total: Int
    public var photos: [PhotoSummary]
}

/// A command as `engine.commands` lists it.
public struct CommandInfo: Codable, Sendable, Identifiable, Hashable {
    public var id: String
    public var label: String
    public var menu: [String]
    public var shortcut: String?
    /// What it takes, in words (`{rating: 0..5, ids?}`); `{}` when nothing.
    public var params: String
    public var enabled: Bool
    public var disabledReason: String?

    enum CodingKeys: String, CodingKey {
        case id, label, menu, shortcut, params, enabled
        case disabledReason = "disabled_reason"
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        label = try c.decode(String.self, forKey: .label)
        menu = try c.decodeIfPresent([String].self, forKey: .menu) ?? []
        shortcut = try c.decodeIfPresent(String.self, forKey: .shortcut)
        params = try c.decodeIfPresent(String.self, forKey: .params) ?? "{}"
        enabled = try c.decodeIfPresent(Bool.self, forKey: .enabled) ?? true
        disabledReason = try c.decodeIfPresent(String.self, forKey: .disabledReason)
    }

    /// Runs without parameters.
    public var takesNothing: Bool {
        params.trimmingCharacters(in: .whitespaces).hasPrefix("{}")
    }
}

/// A develop slider: range, default, step, track style, and the active photo's value.
public struct ControlSpec: Codable, Sendable, Identifiable, Hashable {
    public var id: String
    public var label: String
    public var section: String
    public var min: Double
    public var max: Double
    public var `default`: Double
    public var step: Double
    public var decimals: Int
    public var track: Track
    public var value: Double?

    /// How the slider's track is drawn (`plain`, `centered`, `temp`, `tint`, `gradient`, `hue`,
    /// `sat`, `lum`, `rainbow`).
    public struct Track: Codable, Sendable, Hashable {
        public var kind: String
        public var from: String?
        public var to: String?
        public var band: Int?

        public init(kind: String, from: String? = nil, to: String? = nil, band: Int? = nil) {
            self.kind = kind
            self.from = from
            self.to = to
            self.band = band
        }
    }

    public init(id: String, label: String, section: String, min: Double, max: Double, default: Double, step: Double, decimals: Int, track: Track = Track(kind: "plain"), value: Double? = nil) {
        self.id = id
        self.label = label
        self.section = section
        self.min = min
        self.max = max
        self.default = `default`
        self.step = step
        self.decimals = decimals
        self.track = track
        self.value = value
    }

    /// `v` as the engine shows it: its decimals, and a sign on centred sliders.
    public func format(_ v: Double) -> String {
        let s = String(format: "%.\(Swift.max(0, Swift.min(decimals, 6)))f", v)
        return v > 0 && min < 0 ? "+" + s : s
    }
}
