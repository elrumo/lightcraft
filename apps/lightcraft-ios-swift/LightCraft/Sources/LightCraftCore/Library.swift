import Foundation

// Engine answers about the library: what the grid shows (`library.state`, `library.groups`),
// albums (`albums.list`), counts (`catalog.stats`), label names, history and versions, export
// presets and the server sync. Shapes are the engine's (crates/engine/src/cmd/*.rs); fields the
// app doesn't use are left out, and every field that can be missing is optional.

/// Where the grid's photos come from (`library.source`).
public struct LibrarySource: Codable, Sendable, Hashable {
    /// `all`, `recentlyAdded`, `album`, `recentlyDeleted`, `picks`, `folder`, `missing`, `libraryFolder`.
    public var kind: String
    public var id: UInt64?

    public init(kind: String, id: UInt64? = nil) {
        self.kind = kind
        self.id = id
    }
}

/// The grid's order (`library.sort`).
public struct SortSpec: Codable, Sendable, Hashable {
    /// `captureDate`, `importDate`, `editDate`, `fileName`, `rating`, `fileSize`, `random`.
    public var key: String
    public var ascending: Bool
    /// `auto`, `none`, `day`, `month`, `year`.
    public var group: String?
}

/// The selection: the photos chosen and the one the loupe and the edit tools work on.
public struct SelectionInfo: Codable, Sendable, Hashable {
    public var ids: [UInt64]
    public var active: UInt64?
}

/// `library.state`: the grid's source, filter, sort and selection.
public struct LibraryState: Codable, Sendable {
    public var source: LibrarySource
    public var sourceLabel: String
    /// The filter as the engine has it (`library.filter` merges into it).
    public var filter: JSONValue
    public var sort: SortSpec
    public var selection: SelectionInfo
    public var visibleCount: Int
    public var activeMask: JSONValue?
    public var activeSpot: Int?
    public var undo: String?
    public var redo: String?
    /// Edit settings were copied (Paste can run).
    public var clipboard: Bool

    /// Anything narrows the grid (the default filter is rating ≥ 0, nothing else).
    public var isFiltered: Bool {
        guard case .object(let f) = filter else { return false }
        for (key, value) in f {
            switch (key, value) {
            case ("ratingOp", _), ("subfolders", _): continue
            case ("rating", .int(0)), ("rating", .double(0)): continue
            case ("deleted", .bool(false)): continue
            case ("text", .string("")): continue
            case (_, .null): continue
            case (_, .array(let a)) where a.isEmpty: continue
            default: return true
            }
        }
        return false
    }
}

/// A run of the grid under one date header (`library.groups`).
public struct DateGroup: Codable, Sendable, Hashable, Identifiable {
    public var key: String
    public var label: String
    /// Index of its first photo in the grid.
    public var start: Int
    public var count: Int
    public var id: String { key }
}

/// An album or a folder of albums (`albums.list`, a tree).
public struct AlbumNode: Codable, Sendable, Hashable, Identifiable {
    public var id: UInt64
    public var name: String
    public var folder: Bool
    public var count: Int
    public var cover: UInt64?
    public var children: [AlbumNode]
    public var smart: Bool?
    public var problems: [String]?

    /// A plain album: photos are put in and taken out by hand.
    public var isPlain: Bool { !folder && smart != true }

    /// This node and every node under it, depth first.
    public func flattened() -> [AlbumNode] {
        [self] + children.flatMap { $0.flattened() }
    }
}

/// `catalog.stats`: counts for the albums list.
public struct CatalogStats: Codable, Sendable {
    public var photos: Int
    public var edited: Int
    public var picks: Int
    public var rejects: Int
    public var deleted: Int
    public var albums: Int
    public var byDate: [YearCount]?

    public struct YearCount: Codable, Sendable, Hashable {
        public var year: String
        public var count: Int
        /// `["2026-09", 3]` pairs.
        public var months: [JSONValue]?
    }
}

/// A colour label and the name it has in this library (`label.names`).
public struct LabelName: Codable, Sendable, Hashable, Identifiable {
    public var label: String
    public var name: String
    public var id: String { label }
}

/// `history.list`: the active photo's edit history (oldest first) and its versions.
public struct HistoryList: Codable, Sendable {
    public var history: [String]
    public var versions: [Version]

    public struct Version: Codable, Sendable, Hashable {
        public var name: String
        public var created: String?
    }
}

/// An export preset (`export.presets`): its `params` go to `app.export`.
public struct ExportPreset: Codable, Sendable, Hashable, Identifiable {
    public var name: String
    public var builtin: Bool
    public var params: JSONValue
    public var id: String { name }
}

/// A develop preset (`presets.list`).
public struct PresetInfo: Codable, Sendable, Hashable, Identifiable {
    public var id: String
    public var name: String
    public var group: String
    public var favorite: Bool
    public var builtin: Bool
}

/// `sync.status`: the server sync in full (Settings ▸ Sync, the cloud popover).
public struct SyncStatus: Codable, Sendable, Equatable {
    /// `off` (no server), `signedOut`, `conflict`, `paused`, `error`, `syncing`, `idle`.
    public var state: String
    public var signedIn: Bool
    public var server: String?
    public var user: String?
    public var device: String?
    public var pending: Int?
    public var uploads: Int?
    public var downloads: Int?
    public var uploadsDone: Int?
    public var downloadsDone: Int?
    public var reading: [Int]?
    public var error: String?
    public var conflict: JSONValue?
    public var offlineAlbums: [UInt64]?
    public var storeOriginals: Bool?
    public var originalsBudgetMb: UInt64?
    public var serverPreviews: Bool?
    public var allPreviews: Bool?
    public var paused: Bool?
}

/// One photo in full (`photo.inspect`; snake_case, as the catalog stores it). Only what the Info
/// panel and the loupe's overlays read.
public struct PhotoDetail: Decodable, Sendable {
    public var id: UInt64
    public var fileName: String
    public var format: String?
    public var width: Int
    public var height: Int
    public var fileSize: UInt64?
    public var captured: String?
    public var imported: String?
    public var rating: Int?
    public var flag: String?
    public var label: String?
    public var meta: Meta?
    public var albums: [UInt64]?

    public struct Meta: Decodable, Sendable {
        public var camera: String?
        public var lens: String?
        public var focalMm: Double?
        public var aperture: Double?
        public var shutter: String?
        public var iso: Double?
        public var location: String?
        public var city: String?
        public var state: String?
        public var country: String?
        public var gps: [Double]?
        public var title: String?
        public var caption: String?
        public var copyright: String?
        public var creator: String?
        public var keywords: [String]?

        enum CodingKeys: String, CodingKey {
            case camera, lens, aperture, shutter, iso, location, city, state, country, gps, title, caption, copyright, creator, keywords
            case focalMm = "focal_mm"
        }

        public init(from decoder: any Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            camera = try? c.decodeIfPresent(String.self, forKey: .camera)
            lens = try? c.decodeIfPresent(String.self, forKey: .lens)
            focalMm = try? c.decodeIfPresent(Double.self, forKey: .focalMm)
            aperture = try? c.decodeIfPresent(Double.self, forKey: .aperture)
            // a shutter speed is text ("1/250") in some catalogs and a number of seconds in others
            if let s = try? c.decodeIfPresent(String.self, forKey: .shutter) {
                shutter = s
            } else if let d = try? c.decodeIfPresent(Double.self, forKey: .shutter) {
                shutter = d >= 1 ? String(format: "%.1fs", d) : "1/\(Int((1 / d).rounded()))"
            }
            iso = try? c.decodeIfPresent(Double.self, forKey: .iso)
            location = try? c.decodeIfPresent(String.self, forKey: .location)
            city = try? c.decodeIfPresent(String.self, forKey: .city)
            state = try? c.decodeIfPresent(String.self, forKey: .state)
            country = try? c.decodeIfPresent(String.self, forKey: .country)
            gps = try? c.decodeIfPresent([Double].self, forKey: .gps)
            title = try? c.decodeIfPresent(String.self, forKey: .title)
            caption = try? c.decodeIfPresent(String.self, forKey: .caption)
            copyright = try? c.decodeIfPresent(String.self, forKey: .copyright)
            creator = try? c.decodeIfPresent(String.self, forKey: .creator)
            keywords = try? c.decodeIfPresent([String].self, forKey: .keywords)
        }
    }

    enum CodingKeys: String, CodingKey {
        case id, format, width, height, captured, imported, rating, flag, label, meta, albums
        case fileName = "file_name"
        case fileSize = "file_size"
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(UInt64.self, forKey: .id)
        fileName = (try? c.decode(String.self, forKey: .fileName)) ?? ""
        format = try? c.decodeIfPresent(String.self, forKey: .format)
        width = (try? c.decode(Int.self, forKey: .width)) ?? 0
        height = (try? c.decode(Int.self, forKey: .height)) ?? 0
        fileSize = try? c.decodeIfPresent(UInt64.self, forKey: .fileSize)
        captured = try? c.decodeIfPresent(String.self, forKey: .captured)
        imported = try? c.decodeIfPresent(String.self, forKey: .imported)
        rating = try? c.decodeIfPresent(Int.self, forKey: .rating)
        flag = try? c.decodeIfPresent(String.self, forKey: .flag)
        label = try? c.decodeIfPresent(String.self, forKey: .label)
        meta = try? c.decodeIfPresent(Meta.self, forKey: .meta)
        albums = try? c.decodeIfPresent([UInt64].self, forKey: .albums)
    }
}

/// A month or a year of the grid (`library.blocks`): its photos, the best first.
public struct DateBlockInfo: Codable, Sendable, Hashable, Identifiable {
    public var key: String
    public var label: String
    public var count: Int
    public var photos: [Shown]
    public var id: String { key }

    public struct Shown: Codable, Sendable, Hashable {
        /// Where it is in the grid.
        public var index: Int
        public var id: UInt64
        public var captured: String?

        /// The day of the month it was taken (`12`).
        public var day: String? {
            guard let c = captured, c.count >= 10 else { return nil }
            let d = c.dropFirst(8).prefix(2)
            return String(d.drop { $0 == "0" })
        }
    }
}

/// How the photo as shown (cropped, straightened, flipped) maps to the normalized image coordinates
/// masks, spots, the crop and the pickers take (`view.frame`).
public struct PhotoFrame: Codable, Sendable, Equatable {
    public var aspect: Double
    public var width: Double
    public var height: Double
    /// `[a, b, c, d, e, f]`: (u, v) across the shown image → (a·u + c·v + e, b·u + d·v + f).
    public var toNorm: [Double]
    public var fromNorm: [Double]
    /// The crop on the whole photo (the crop tool's frame).
    public var crop: Crop?

    public struct Crop: Codable, Sendable, Equatable {
        /// `[x0, y0, x1, y1]` in the straightened frame.
        public var rect: [Double]
        public var angle: Double
        /// Top-left, top-right, bottom-right, bottom-left of the unflipped crop, in image coordinates.
        public var quad: [[Double]]
        /// Image coordinates → the straightened frame `crop.drag` takes.
        public var toStraight: [Double]
        public var flipH: Bool
        public var flipV: Bool

        public func straight(x: Double, y: Double) -> (x: Double, y: Double) {
            let p = PhotoFrame.apply(toStraight, x, y)
            return (p.0, p.1)
        }
    }

    /// A point across the shown image (0…1, top-left first) in image coordinates.
    public func norm(u: Double, v: Double) -> (x: Double, y: Double) {
        Self.apply(toNorm, u, v)
    }

    /// Image coordinates as a point across the shown image.
    public func shown(x: Double, y: Double) -> (u: Double, v: Double) {
        let p = Self.apply(fromNorm, x, y)
        return (p.0, p.1)
    }

    static func apply(_ m: [Double], _ u: Double, _ v: Double) -> (Double, Double) {
        guard m.count == 6 else { return (u, v) }
        return (m[0] * u + m[2] * v + m[4], m[1] * u + m[3] * v + m[5])
    }
}
