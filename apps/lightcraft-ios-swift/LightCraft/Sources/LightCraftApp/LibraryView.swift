#if os(iOS)
import LightCraftCore
import SwiftUI

/// The library grid: square thumbnails edge to edge, the photo's picture filling its square.
struct LibraryView: View {
    @Environment(AppModel.self) private var model
    private let columns = [GridItem(.adaptive(minimum: 104, maximum: 220), spacing: 2)]

    var body: some View {
        ScrollView {
            LazyVGrid(columns: columns, spacing: 2) {
                ForEach(model.library.photos) { photo in
                    NavigationLink(value: Route.photo(photo.id)) {
                        ThumbnailCell(photo: photo)
                    }
                    .buttonStyle(.plain)
                }
            }
        }
        .navigationTitle(model.opened?.demo == true ? "Demo Library" : "All Photos")
        .toolbarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    Button("All Commands", systemImage: "command") { model.path.append(.commands) }
                } label: {
                    Label("More", systemImage: "ellipsis")
                }
            }
        }
        .overlay {
            if model.library.photos.isEmpty && !model.library.loading {
                ContentUnavailableView("No Photos", systemImage: "photo.on.rectangle", description: Text("This library has no photos yet."))
            }
        }
    }
}

/// One square of the grid.
struct ThumbnailCell: View {
    let photo: PhotoSummary
    @Environment(AppModel.self) private var model
    /// Pixels asked for on the long edge (the engine rounds it to its own sizes).
    private let edge = 384

    var body: some View {
        Color(uiColor: .secondarySystemBackground)
            .aspectRatio(1, contentMode: .fit)
            .overlay {
                if let picture = model.images.thumb(photo.id) {
                    Image(decorative: picture.image, scale: 1)
                        .resizable()
                        .scaledToFill()
                }
            }
            .clipped()
            .contentShape(Rectangle())
            // asked again when the library changes (an edit): the engine answers only what changed
            .task(id: model.state.catalogRevision) { model.requestThumb(photo.id, edge: edge) }
            .onAppear { model.thumbAppeared(photo.id) }
            .onDisappear { model.thumbDisappeared(photo.id) }
            .accessibilityLabel(photo.title ?? photo.fileName)
    }
}
#endif
