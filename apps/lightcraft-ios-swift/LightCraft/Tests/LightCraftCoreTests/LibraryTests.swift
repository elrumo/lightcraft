import Foundation
import Testing

@testable import LightCraftCore

struct LibraryTests {
    /// Signing in to sync shrank the library before the grid's date groups caught up: a group's
    /// range started past its end and the grid crashed. It is cut to the grid now.
    @Test func aStaleGroupGivesOnlyTheIndexesTheGridHas() {
        let g = DateGroup(key: "2026-08", label: "August 2026", start: 30, count: 5)
        #expect(g.indices(in: 24) == 24..<24)
        #expect(g.indices(in: 32) == 30..<32)
        #expect(g.indices(in: 100) == 30..<35)
        #expect(DateGroup(key: "x", label: "x", start: -3, count: 2).indices(in: 10) == 0..<2)
        #expect(DateGroup(key: "x", label: "x", start: 2, count: Int.max).indices(in: 10) == 2..<10)
    }

    /// `sync.status` as the engine answered it once signed in (2026-10-14): `device` is a number,
    /// which the app read as a string, so Settings ▸ Sync showed the sign-in form while signed in.
    @Test func aSignedInStatusDecodes() throws {
        let reply = """
        {"allPreviews": false, "conflict": null, "cursor": 0, "device": 1, "downloads": 0, "downloadsDone": 0, "error": null,
         "library": "b704be8469829f9f610cea51742f2c93", "merged": null, "offlineAlbums": [], "offlinePhotos": [],
         "originalsBudgetMb": null, "paused": false, "pending": 0, "reading": null, "server": "http://127.0.0.1:18471",
         "serverPreviews": true, "signedIn": true, "state": "idle", "storeOriginals": false, "uploads": 0, "uploadsDone": 0,
         "user": "swifttest"}
        """
        let s = try JSONDecoder().decode(SyncStatus.self, from: Data(reply.utf8))
        #expect(s.signedIn && s.device == 1 && s.user == "swifttest")
    }

    /// `sync.status` while signing out waits for what is left to be sent (2026-10-15): `unsent` is `null` before the first
    /// count, and `library` tells Settings ▸ Sync whether signing out removes an account's library (it asks first) or
    /// a library that never joined one (it signs out at once).
    @Test func aSigningOutStatusDecodes() throws {
        let reply = """
        {"allPreviews": false, "conflict": null, "cursor": 7, "device": 1, "downloads": 0, "downloadsDone": 0,
         "error": "can't reach the server", "leaving": true, "library": "b704be8469829f9f610cea51742f2c93", "merged": null,
         "offlineAlbums": [], "offlinePhotos": [], "originalsBudgetMb": null, "paused": false, "pending": 3, "reading": null,
         "refused": 0, "server": "http://127.0.0.1:18471", "serverPreviews": true, "signedIn": true, "state": "signingOut",
         "storeOriginals": false, "unsent": null, "uploads": 2, "uploadsDone": 0, "user": "swifttest"}
        """
        let s = try JSONDecoder().decode(SyncStatus.self, from: Data(reply.utf8))
        #expect(s.state == "signingOut" && s.signedIn && s.leaving == true)
        #expect(s.unsent == nil && s.refused == 0 && s.pending == 3)
        #expect(!(s.library ?? "").isEmpty)
        // a library that never joined one has an empty id; a library with no server has none at all
        let never = try JSONDecoder().decode(SyncStatus.self, from: Data(#"{"state": "signedOut", "signedIn": false, "library": ""}"#.utf8))
        #expect((never.library ?? "").isEmpty && never.leaving == nil)
        let off = try JSONDecoder().decode(SyncStatus.self, from: Data(#"{"state": "off", "signedIn": false}"#.utf8))
        #expect(off.library == nil && off.unsent == nil)
        // the cloud's summary says the same state
        let summary = try JSONDecoder().decode(
            SyncSummary.self,
            from: Data(#"{"state": "signingOut", "server": "http://x", "pending": 3, "uploads": 2, "downloads": 0, "reading": null, "error": null}"#.utf8))
        #expect(summary.state == "signingOut" && summary.pending == 3 && summary.uploads == 2)
    }
}
