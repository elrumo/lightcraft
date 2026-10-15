#if os(macOS)
import LightCraftCore
import SwiftUI

/// Masking as Lightroom lays it out: the photo's masks (and the selected one's parts) at the top of
/// the panel, the brush when it is in hand, then the selected mask's controls stacked and
/// collapsible: Mask (amount, invert, its parts' ranges, refine), Light, Color, Effects, Detail,
/// Optics. Not the phone's column over the photo and row of group chips.
struct MacMaskingPanel: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let mask = MaskList.selected(model)
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                MaskColumn(wrap: true)
                    .padding(16)
                if model.editor.maskTool == "brush" {
                    BrushControls()
                    Divider().padding(.top, 8)
                }
                if let mask {
                    MacSection(id: "mask.mask", title: "Mask", open: true) { MaskSettings(mask: mask) }
                    ForEach(MaskGroupRow.groups.filter { $0.id != "mask" }, id: \.id) { group in
                        MacSection(id: "mask.\(group.id)", title: LocalizedStringKey(group.label), open: group.id == "light") {
                            MaskAdjustments(mask: mask["id"], ids: MaskGroupRow.ids[group.id] ?? [])
                        }
                    }
                } else if MaskList.all(model).isEmpty {
                    Text("Tap + to make a mask: the subject, the sky, a gradient, a brush…")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .padding(.horizontal, 16)
                }
            }
        }
        .animation(.snappy, value: mask == nil)
    }
}
#endif
