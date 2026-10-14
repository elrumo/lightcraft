#if os(macOS)
import LightCraftCore
import SwiftUI

// The small controls the Mac's bars (under the loupe, under the grid) share.

/// A bar's icon button: dim, with a pill behind it when it is on.
struct MacBarButton: View {
    let symbol: String
    let title: LocalizedStringKey
    var on = false
    var tint: Color?
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: 13))
                .frame(width: 26, height: 24)
                .background(on ? Color.primary.opacity(0.18) : .clear, in: .rect(cornerRadius: 6))
                .foregroundStyle(tint ?? (on ? Color.primary : Color.secondary))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help(Text(title))
        .accessibilityLabel(Text(title))
        .accessibilityAddTraits(on ? .isSelected : [])
    }
}

/// Five stars for `ids`: a click rates them by it; on the star they have, takes the rating away.
struct MacStars: View {
    @Environment(AppModel.self) private var model
    let rating: Int
    let ids: [UInt64]

    var body: some View {
        HStack(spacing: 1) {
            ForEach(1...5, id: \.self) { n in
                Button {
                    model.perform("photo.rate", ["ids": AppModel.ids(ids), "rating": .int(Int64(n == rating ? 0 : n))])
                } label: {
                    Image(systemName: n <= rating ? "star.fill" : "star")
                        .font(.system(size: 13))
                        .foregroundStyle(n <= rating ? Color.yellow : .secondary)
                        .frame(width: 20, height: 24)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("\(n) stars")
                .accessibilityAddTraits(n == rating ? .isSelected : [])
            }
        }
        .disabled(ids.isEmpty)
    }
}

/// Pick and Reject for `ids`; a click on the one they have unflags them.
struct MacFlags: View {
    @Environment(AppModel.self) private var model
    let flag: String?
    let ids: [UInt64]

    var body: some View {
        let set = { (flag: String) in
            model.perform("photo.flag", ["ids": AppModel.ids(ids), "flag": .string(flag)])
        }
        HStack(spacing: 2) {
            MacBarButton(symbol: "flag.fill", title: "Pick", on: flag == "pick") { set(flag == "pick" ? "none" : "pick") }
            MacBarButton(symbol: "flag.slash.fill", title: "Reject", on: flag == "reject", tint: flag == "reject" ? .red : nil) {
                set(flag == "reject" ? "none" : "reject")
            }
        }
        .disabled(ids.isEmpty)
    }
}
#endif
