import Foundation
import Synchronization

/// The LightCraft engine, running on its own thread in the Rust library. Ask it with
/// [`request`](Engine/request(_:_:)) (any engine command through `engine.execute`, rendering
/// through `render.*`); what changes arrives on [`events`](Engine/events), pictures included.
/// Safe to use from any task or thread.
public final class Engine: Sendable {
    public let handle: UInt64
    /// What the engine reports: pictures, state changes, background tasks, messages, automation
    /// requests. One consumer (the app model).
    public let events: AsyncStream<EngineEvent>
    private let replies: Replies
    private let pump: EventPump
    private let nextID = Atomic<UInt64>(1)

    private init(handle: UInt64) {
        self.handle = handle
        let (stream, sink) = AsyncStream<EngineEvent>.makeStream(bufferingPolicy: .unbounded)
        let replies = Replies()
        events = stream
        self.replies = replies
        pump = EventPump(engine: handle, replies: replies, sink: sink)
        pump.start()
    }

    /// Start an engine (opening a library takes a moment: done off the caller's thread).
    public static func open(_ config: OpenConfig) async throws -> (Engine, Opened) {
        guard Bridge.abiVersion == Bridge.expectedAbiVersion else {
            throw ErrorBody(code: "abi", message: "the engine library speaks ABI \(Bridge.abiVersion), this app \(Bridge.expectedAbiVersion)")
        }
        let body = try JSONEncoder().encode(config)
        let result = await Task.detached(priority: .userInitiated) { Bridge.open(config: body) }.value
        let opened = try JSONDecoder().decode(Opened.self, from: result.get())
        return (Engine(handle: opened.engine), opened)
    }

    /// Run `method` and wait for its answer.
    @discardableResult
    public func request(_ method: String, _ params: JSONValue = [:]) async throws -> JSONValue {
        let id = nextID.wrappingAdd(1, ordering: .relaxed).oldValue
        let body = try JSONEncoder().encode(Request(id: id, method: method, params: params))
        return try await withCheckedThrowingContinuation { continuation in
            replies.add(id, continuation)
            if let refused = Bridge.submit(handle, body) {
                replies.resolve(id, .failure(refused))
            }
        }
    }

    /// Queue `method` without waiting for its answer (render requests: their pictures come as
    /// events). An error answer comes as a `message` event.
    public func send(_ method: String, _ params: JSONValue = [:]) {
        let id = nextID.wrappingAdd(1, ordering: .relaxed).oldValue
        guard let body = try? JSONEncoder().encode(Request(id: id, method: method, params: params)) else { return }
        _ = Bridge.submit(handle, body)
    }

    /// An engine command (`Session::execute`): `execute("develop.set", ["control": "light.exposure", "value": 0.5])`.
    @discardableResult
    public func execute(_ command: String, _ params: JSONValue = [:]) async throws -> JSONValue {
        try await request("engine.execute", ["command": .string(command), "params": params])
    }

    /// An engine command whose answer is decoded as `T`.
    public func execute<T: Decodable>(_ command: String, _ params: JSONValue = [:], as type: T.Type) async throws -> T {
        try await execute(command, params).decode(T.self)
    }

    /// Every command, engine and host, with whether it can run now.
    public func commands() async throws -> [CommandInfo] {
        try await request("engine.commands").decode([CommandInfo].self)
    }

    /// The develop sliders of `section` (`light`, `color`, `effects`…) with the active photo's values.
    public func controls(section: String? = nil) async throws -> [ControlSpec] {
        let params: JSONValue = section.map { ["section": .string($0)] } ?? [:]
        return try await execute("develop.controls", params, as: [ControlSpec].self)
    }

    /// Save, stop the engine and its pump (waits a few seconds at most, off the caller's thread).
    public func close() async {
        pump.stop()
        let handle = handle
        await Task.detached { Bridge.close(handle) }.value
    }
}
