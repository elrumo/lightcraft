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
}
