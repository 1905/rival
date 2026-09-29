import RivalKit
import SwiftUI

/// A segmented control in the phosphor look: SF Mono labels, a dim frame, and
/// the selected segment on the accent bar with black text (the TUI's tabs).
/// The native `Picker(.segmented)` ignores custom fonts on macOS.
///
/// `fill` stretches the segments across the available width.
struct Segmented<Value: Hashable>: View {
    @Binding var selection: Value
    let options: [Value]
    var fill = false
    let label: (Value) -> String

    var body: some View {
        HStack(spacing: 0) {
            ForEach(Array(options.enumerated()), id: \.element) { index, value in
                let on = value == selection
                Button {
                    selection = value
                } label: {
                    Text(label(value))
                        .font(Mono.small)
                        .fontWeight(on ? .semibold : .regular)
                        .lineLimit(1)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 4)
                        .frame(maxWidth: fill ? .infinity : nil)
                        .foregroundStyle(on ? Theme.accent : Theme.fg)
                        .background(on ? Theme.selectionBg : Color.clear)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .overlay(alignment: .leading) {
                    if index > 0 { Rectangle().fill(Theme.dim).frame(width: 1) }
                }
                .accessibilityAddTraits(on ? .isSelected : [])
            }
        }
        .fixedSize(horizontal: !fill, vertical: true)
        .clipShape(RoundedRectangle(cornerRadius: 4))
        .overlay(RoundedRectangle(cornerRadius: 4).stroke(Theme.dim, lineWidth: 1))
    }
}
