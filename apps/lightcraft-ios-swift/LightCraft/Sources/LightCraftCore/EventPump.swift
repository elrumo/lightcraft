import Foundation
import Synchronization

/// Requests waiting for their replies, resolved by the pump's thread.
final class Replies: Sendable {
    private let waiting = Mutex<[UInt64: CheckedContinuation<JSONValue, any Error>]>([:])

    func add(_ id: UInt64, _ continuation: CheckedContinuation<JSONValue, any Error>) {
        waiting.withLock { $0[id] = continuation }
    }

    /// Resolve `id`'s waiter; false when nobody waits for it (a fire-and-forget request).
    @discardableResult
    func resolve(_ id: UInt64, _ result: Result<JSONValue, ErrorBody>) -> Bool {
        guard let c = waiting.withLock({ $0.removeValue(forKey: id) }) else { return false }
        switch result {
        case .success(let v): c.resume(returning: v)
        case .failure(let e): c.resume(throwing: e)
        }
        return true
    }

    /// Fail everyone still waiting (the engine closed).
    func failAll(_ error: ErrorBody) {
        let all = waiting.withLock { w in
            let all = Array(w.values)
            w.removeAll()
            return all
        }
        for c in all {
            c.resume(throwing: error)
        }
    }
}

/// A thread of its own that blocks in `lc_poll`, so the cooperative thread pool never does: it
/// decodes the events, fetches the pixels of pictures (off the main thread), answers waiting
/// requests, and hands the rest to `sink`.
final class EventPump: Sendable {
    private let engine: UInt64
    private let replies: Replies
    private let sink: AsyncStream<EngineEvent>.Continuation
    private let stopped = Atomic<Bool>(false)

    init(engine: UInt64, replies: Replies, sink: AsyncStream<EngineEvent>.Continuation) {
        self.engine = engine
        self.replies = replies
        self.sink = sink
    }

    func start() {
        let thread = Thread { [self] in self.run() }
        thread.name = "lc-pump"
        thread.qualityOfService = .userInitiated
        thread.start()
    }

    /// Stop polling (the thread ends within a poll).
    func stop() {
        stopped.store(true, ordering: .relaxed)
        Bridge.wake(engine)
    }

    private func run() {
        let decoder = JSONDecoder()
        while !stopped.load(ordering: .relaxed) {
            guard case .success(let data) = Bridge.poll(engine, timeoutMs: 250) else {
                // the engine is gone (closed): nothing more will come
                replies.failAll(ErrorBody(code: "closed", message: "the engine has stopped"))
                break
            }
            guard let events = try? decoder.decode([WireEvent].self, from: data) else { continue }
            for wire in events {
                route(wire.event)
            }
        }
        sink.finish()
    }

    private func route(_ event: EngineEvent) {
        switch event {
        case .reply(let id, let result):
            if !replies.resolve(id, result), case .failure(let e) = result {
                sink.yield(.message(text: e.message, level: "error"))
            }
        case .image(let info, _):
            // a newer picture of the slot may already have replaced this one: then nil
            sink.yield(.image(info, Bridge.image(token: info.token)))
        case .closed:
            replies.failAll(ErrorBody(code: "closed", message: "the engine has stopped"))
            sink.yield(.closed)
        default:
            sink.yield(event)
        }
    }
}
