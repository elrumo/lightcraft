import Foundation
import Testing

@testable import LightCraftCore

/// The protocol's shapes, as the Rust side writes them (apps/lightcraft-ios-swift/src/fixtures.rs
/// → Fixtures/protocol.json): a change on one side the other doesn't follow fails here.
struct ProtocolTests {
    private func fixtures() throws -> JSONValue {
        let url = try #require(Bundle.module.url(forResource: "protocol", withExtension: "json", subdirectory: "Fixtures"))
        return try JSONDecoder().decode(JSONValue.self, from: Data(contentsOf: url))
    }

    private func events() throws -> [EngineEvent] {
        let data = try JSONEncoder().encode(try fixtures()["events"])
        return try JSONDecoder().decode([WireEvent].self, from: data).map(\.event)
    }

    @Test func everyEventKindDecodes() throws {
        let all = try events()
        #expect(all.count == 10)
        for event in all {
            if case .unknown(let kind) = event {
                Issue.record("an event of kind \(kind) didn't decode")
            }
        }
    }

    @Test func repliesCarryResultsOrErrors() throws {
        let all = try events()
        guard case .reply(let id, .success(let result)) = all[0] else { Issue.record("\(all[0])"); return }
        #expect(id == 1)
        #expect(result["total"].int == 0)
        guard case .reply(2, .failure(let error)) = all[1] else { Issue.record("\(all[1])"); return }
        #expect(error.code == "unknownCommand")
    }

    @Test func picturesKeepTheirFacts() throws {
        let all = try events()
        guard case .image(let main, nil) = all[2] else { Issue.record("\(all[2])"); return }
        #expect(main.slot == "main")
        #expect(main.key == 18_446_744_073_709_551_000)
        #expect(main.draft && !main.isFinal)
        #expect(main.histogram?.total == 1)
        guard case .image(let thumb, nil) = all[3] else { Issue.record("\(all[3])"); return }
        #expect(thumb.quick == "embedded")
        #expect(thumb.histogram == nil)
    }

    @Test func stateActivityAndAutomationDecode() throws {
        let all = try events()
        guard case .state(let state) = all[5] else { Issue.record("\(all[5])"); return }
        #expect(state.active == 7 && state.undo == "Exposure" && state.redo == nil)
        guard case .activity(let tasks) = all[6] else { Issue.record("\(all[6])"); return }
        #expect(tasks.first?.unit == "count" && tasks.first?.ageMs == 1200)
        guard case .uiRequest(4, "ui.screenshot", let params) = all[8] else { Issue.record("\(all[8])"); return }
        #expect(params["path"].string == "/tmp/shot.png")
        guard case .closed = all[9] else { Issue.record("\(all[9])"); return }
    }

    @Test func requestsAndConfigEncodeAsTheEngineReadsThem() throws {
        let f = try fixtures()
        let request = try f["request"].decode(Request.self)
        #expect(try JSONValue.encode(request) == f["request"])
        let config = try f["openConfig"].decode(OpenConfig.self)
        #expect(config.demo && config.deviceName == "iPhone")
        // the engine refuses unknown fields: what Swift writes must be exactly what it knows
        // (Swift leaves out what is nil, Rust writes it as null: the same to the engine)
        #expect(withoutNulls(try JSONValue.encode(config)) == withoutNulls(f["openConfig"]))
    }

    private func withoutNulls(_ v: JSONValue) -> JSONValue {
        guard case .object(let o) = v else { return v }
        return .object(o.filter { !$0.value.isNull })
    }

    @Test func unknownAndMalformedEventsDontStopTheRest() throws {
        let data = Data(#"[{"kind": "fromTheFuture"}, {"kind": "state"}, {"kind": "closed"}]"#.utf8)
        let decoded = try JSONDecoder().decode([WireEvent].self, from: data).map(\.event)
        guard case .unknown("fromTheFuture") = decoded[0], case .unknown("state") = decoded[1], case .closed = decoded[2] else {
            Issue.record("\(decoded)")
            return
        }
    }
}
