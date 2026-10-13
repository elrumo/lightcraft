#if os(iOS)
import LightCraftCore
import SwiftUI

/// The navigation root: the grid, and the photo and All Commands screens pushed over it.
struct RootView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        NavigationStack(path: $model.path) {
            Group {
                if let error = model.startError {
                    ContentUnavailableView("LightCraft couldn't start", systemImage: "exclamationmark.triangle", description: Text(error))
                } else if model.engine == nil {
                    ProgressView("Opening your library…")
                } else {
                    LibraryView()
                }
            }
            .navigationDestination(for: Route.self) { route in
                switch route {
                case .photo(let id): LoupeView(photoID: id)
                case .commands: AllCommandsView()
                }
            }
        }
        .overlay(alignment: .bottom) {
            VStack(spacing: 8) {
                ForEach(model.activity) { task in
                    ActivityRow(task: task)
                }
                if let message = model.message {
                    Text(message)
                        .font(.callout)
                        .padding(.horizontal, 16)
                        .padding(.vertical, 10)
                        .glassEffect()
                        .onTapGesture { model.message = nil }
                        .task(id: message) {
                            try? await Task.sleep(for: .seconds(4))
                            if model.message == message { model.message = nil }
                        }
                }
            }
            .padding(.bottom, 24)
            .animation(.snappy, value: model.message)
        }
    }
}

/// A background task (import, export, sync…) while it runs.
struct ActivityRow: View {
    let task: ActivityTask
    @Environment(AppModel.self) private var model

    var body: some View {
        HStack(spacing: 12) {
            if task.total > 0 {
                ProgressView(value: Double(task.done), total: Double(task.total))
                    .frame(width: 80)
            } else {
                ProgressView()
            }
            Text(task.label).font(.footnote).lineLimit(1)
            if task.cancellable && !task.cancelling {
                Button("Cancel", systemImage: "xmark.circle.fill") {
                    Task { await model.run("activity.cancel", ["id": .int(Int64(clamping: task.id))]) }
                }
                .labelStyle(.iconOnly)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .glassEffect()
    }
}
#endif
