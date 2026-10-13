#if os(iOS)
import LightCraftCore
import SwiftUI

/// Choose the edit settings to copy from the photo, or to paste onto the photos chosen: a checklist
/// of the settings groups (`develop.copy {groups}`, `develop.paste {groups}`).
struct CopySettingsSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    /// Paste onto these photos; `nil`: copy from the photo.
    let pasteOnto: [UInt64]?
    @State private var chosen: Set<String> = Set(Self.groups.map(\.0)).subtracting(["crop", "spots", "redEye", "masks"])

    /// The settings groups, as `develop.copy` names them.
    static let groups: [(String, LocalizedStringKey)] = [
        ("profile", "Profile"), ("treatment", "Treatment"), ("whiteBalance", "White Balance"), ("light", "Light"),
        ("toneCurve", "Tone Curve"), ("color", "Color"), ("colorMixer", "Color Mixer"), ("colorGrading", "Color Grading"),
        ("effects", "Effects"), ("vignette", "Vignette"), ("grain", "Grain"), ("detail", "Detail"), ("optics", "Optics"),
        ("geometry", "Geometry"), ("crop", "Crop"), ("masks", "Masks"), ("spots", "Remove"), ("redEye", "Red Eye"),
        ("lensBlur", "Lens Blur"), ("calibration", "Calibration"),
    ]

    var body: some View {
        NavigationStack {
            List {
                Section {
                    ForEach(Self.groups, id: \.0) { id, label in
                        Toggle(label, isOn: Binding(get: { chosen.contains(id) }, set: { on in
                            if on { chosen.insert(id) } else { chosen.remove(id) }
                        }))
                    }
                } header: {
                    HStack {
                        Button("All") { chosen = Set(Self.groups.map(\.0)) }
                        Spacer()
                        Button("None") { chosen = [] }
                    }
                    .font(.subheadline)
                    .textCase(nil)
                }
            }
            .navigationTitle(pasteOnto == nil ? "Copy Settings" : "Paste Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel", systemImage: "xmark") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button(pasteOnto == nil ? "Copy" : "Paste") {
                        Task {
                            let groups = JSONValue.array(Self.groups.map(\.0).filter(chosen.contains).map { .string($0) })
                            if let ids = pasteOnto {
                                await model.run("develop.paste", ["ids": AppModel.ids(ids), "groups": groups])
                            } else if await model.run("develop.copy", ["groups": groups]) != nil {
                                model.show(String(localized: "Edit settings copied"))
                            }
                            dismiss()
                        }
                    }
                    .disabled(chosen.isEmpty)
                }
            }
        }
    }
}
#endif
