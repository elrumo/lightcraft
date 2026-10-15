import SwiftUI

/// The engine wouldn't start: the reason (on a Mac, usually that the library is open in another
/// LightCraft, the egui app holds a lock on it, or that its folder can't be read) and the way on,
/// with the egui app's words for it.
struct StartErrorView: View {
    @Environment(AppModel.self) private var model
    let error: String

    var body: some View {
        ContentUnavailableView {
            Label("LightCraft couldn't start", systemImage: "exclamationmark.triangle")
        } description: {
            Text(error)
        } actions: {
            Button("Try Again") { model.retryStart() }
                .buttonStyle(.borderedProminent)
            #if os(macOS)
            Button("Choose Another Library…") { chooseLibrary() }
            #endif
            Button("Continue Without Saving") { model.retryStart(demo: true) }
        }
    }

    #if os(macOS)
    /// A library is a folder: an existing one, or an empty one to start a library in.
    private func chooseLibrary() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.canCreateDirectories = true
        if panel.runModal() == .OK, let url = panel.url { model.retryStart(library: url.path) }
    }
    #endif
}
