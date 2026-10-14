#if os(macOS)
import SwiftUI

/// Settings (⌘,), as a Mac has them: a window of tabs, each one of the pages the iPhone lists.
struct MacSettings: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        TabView(selection: $model.settingsTab) {
            Tab("General", systemImage: "gear", value: "general") { page("general") }
            Tab("Import", systemImage: "square.and.arrow.down", value: "import") { page("import") }
            Tab("Performance", systemImage: "gauge.with.dots.needle.67percent", value: "performance") { page("performance") }
            Tab("Interface", systemImage: "paintbrush", value: "interface") { page("interface") }
            Tab("Sync", systemImage: "arrow.triangle.2.circlepath.icloud", value: "sync") { page("sync") }
            Tab("Faces", systemImage: "person.crop.square", value: "faces") { page("faces") }
            Tab("AI Models", systemImage: "cpu", value: "models") { page("models") }
        }
        .scenePadding()
        .frame(width: 640, height: 520)
    }

    /// A page in a stack of its own: Sync and Faces open further pages (storage, face models).
    private func page(_ name: String) -> some View {
        NavigationStack {
            SettingsView.page(name)
                .formStyle(.grouped)
                .navigationDestination(for: String.self) { SettingsView.page($0) }
        }
    }
}
#endif
