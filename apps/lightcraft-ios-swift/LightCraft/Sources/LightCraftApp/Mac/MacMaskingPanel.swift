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
        MacPanel(title: "Masking") {
            MacGroup(title: "Masks") { MaskColumn(wrap: true) }
            if model.editor.maskTool == "brush" {
                MacGroup(title: "Brush") { MacBrushControls() }
            }
            if let mask {
                MacSection(id: "mask.mask", title: "Mask", open: true) { MaskSettings(mask: mask) }
                ForEach(MaskGroupRow.groups.filter { $0.id != "mask" }, id: \.id) { group in
                    MacSection(id: "mask.\(group.id)", title: LocalizedStringKey(group.label), open: group.id == "light") {
                        MaskAdjustments(mask: mask["id"], ids: MaskGroupRow.ids[group.id] ?? [])
                    }
                }
            } else if MaskList.all(model).isEmpty {
                MacGroup { MacHint(text: Text("Tap + to make a mask: the subject, the sky, a gradient, a brush…")) }
            }
        }
        .animation(.snappy, value: mask == nil)
    }
}

/// The brush in hand: paint or erase, Auto Mask, its size, feather and flow.
private struct MacBrushControls: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var e = model.editor
        MacSegmented(selection: $e.brushErase, options: [(false, "Paint"), (true, "Erase")])
            .padding(.bottom, 4)
        MacSliderRow(spec: ControlSpec(id: "brush.size", label: "Size", section: "brush", min: 1, max: 100, default: 12, step: 1, decimals: 0),
                     value: e.brushSize * 400, set: { e.brushSize = $0 / 400 }, editing: { _ in }, reset: { e.brushSize = 0.03 })
        MacSliderRow(spec: ControlSpec(id: "brush.feather", label: "Feather", section: "brush", min: 0, max: 100, default: 50, step: 1, decimals: 0),
                     value: e.brushFeather, set: { e.brushFeather = $0 }, editing: { _ in }, reset: { e.brushFeather = 50 })
        MacSliderRow(spec: ControlSpec(id: "brush.flow", label: "Flow", section: "brush", min: 1, max: 100, default: 100, step: 1, decimals: 0),
                     value: e.brushFlow, set: { e.brushFlow = $0 }, editing: { _ in }, reset: { e.brushFlow = 100 })
        MacCheck(title: "Auto Mask", isOn: $e.brushAutoMask)
        MacButton(title: "Done", wide: false) { e.maskTool = nil }
            .padding(.top, 4)
    }
}
#endif
