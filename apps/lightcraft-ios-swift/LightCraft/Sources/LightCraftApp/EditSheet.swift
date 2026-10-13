#if os(iOS)
import LightCraftCore
import SwiftUI

/// The Edit tool's Light group: one slider per control the engine describes
/// (`develop.controls {section: "light"}`), so a new control appears here without a change.
struct EditSheet: View {
    @Environment(AppModel.self) private var model
    @State private var edit = EditModel()

    var body: some View {
        NavigationStack {
            List {
                Section("Light") {
                    ForEach(edit.controls) { spec in
                        ControlRow(
                            spec: spec,
                            value: edit.value(spec),
                            set: { edit.set(spec, $0, model) },
                            editing: { edit.editing(spec, $0, model) },
                            reset: { edit.reset(spec, model) }
                        )
                    }
                }
            }
            .listStyle(.insetGrouped)
            .navigationTitle("Edit")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done", systemImage: "checkmark") { model.editing = false }
                }
            }
        }
        // the values follow the photo (undo, another device): reloaded when the library changes
        .task(id: model.state.catalogRevision) { await edit.load(model) }
    }
}

/// One slider: label, value, and the track. Double tap resets it to its default.
struct ControlRow: View {
    let spec: ControlSpec
    let value: Double
    let set: (Double) -> Void
    let editing: (Bool) -> Void
    let reset: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack {
                Text(LocalizedStringKey(spec.label))
                Spacer()
                Text(spec.format(value))
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
            }
            .font(.subheadline)
            if spec.min < spec.max {
                let binding = Binding(get: { value }, set: { set($0) })
                if spec.step > 0 {
                    Slider(value: binding, in: spec.min...spec.max, step: spec.step, onEditingChanged: editing)
                } else {
                    Slider(value: binding, in: spec.min...spec.max, onEditingChanged: editing)
                }
            }
        }
        .contentShape(Rectangle())
        .onTapGesture(count: 2, perform: reset)
        .accessibilityElement(children: .combine)
    }
}

/// A slider drag as the engine wants it: `develop.beginInteraction`, `develop.set` with at most
/// one in flight (the newest value wins) and a draft render after each, then the last value and
/// `develop.endInteraction` (one undo step), and the full render.
@Observable
final class EditModel {
    private(set) var controls: [ControlSpec] = []
    /// Values moved here and not yet reloaded from the engine.
    private var local: [String: Double] = [:]
    private var dragging: String?
    private var dirty: String?
    private var inFlight = false

    func load(_ app: AppModel) async {
        guard dragging == nil, let engine = app.engine else { return }
        if let specs = try? await engine.controls(section: "light"), dragging == nil {
            controls = specs
            local = [:]
        }
    }

    func value(_ spec: ControlSpec) -> Double {
        local[spec.id] ?? spec.value ?? spec.default
    }

    func editing(_ spec: ControlSpec, _ began: Bool, _ app: AppModel) {
        guard let engine = app.engine else { return }
        if began {
            dragging = spec.id
            engine.send("engine.execute", ["command": "develop.beginInteraction", "params": ["label": .string(spec.label)]])
        } else {
            // requests run in order: the last value, then the end of the drag (one undo step)
            engine.send("engine.execute", ["command": "develop.set", "params": ["control": .string(spec.id), "value": .double(value(spec))]])
            engine.send("engine.execute", ["command": "develop.endInteraction"])
            dragging = nil
            dirty = nil
        }
    }

    func set(_ spec: ControlSpec, _ v: Double, _ app: AppModel) {
        local[spec.id] = v
        guard dragging == spec.id else {
            // a tap on the track (no drag): one step of its own
            Task { await app.run("develop.set", ["control": .string(spec.id), "value": .double(v)]) }
            return
        }
        dirty = spec.id
        pump(app)
    }

    func reset(_ spec: ControlSpec, _ app: AppModel) {
        local[spec.id] = spec.default
        Task { await app.run("develop.set", ["control": .string(spec.id), "value": .double(spec.default)]) }
    }

    /// Send the newest value if nothing is in flight; a draft render follows each.
    private func pump(_ app: AppModel) {
        guard !inFlight, let id = dirty, let engine = app.engine else { return }
        inFlight = true
        dirty = nil
        let v = local[id] ?? 0
        Task {
            _ = try? await engine.execute("develop.set", ["control": .string(id), "value": .double(v)])
            inFlight = false
            if dragging != nil {
                app.requestDraft()
                pump(app)
            }
        }
    }
}
#endif
