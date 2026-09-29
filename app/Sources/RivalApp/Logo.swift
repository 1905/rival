import RivalKit
import SwiftUI

/// The TUI's ASCII logo, one colour per cell on a 45° violet → cyan → green
/// gradient. Every line is padded to the banner width so the gradient lines up.
struct Logo: View {
    @MainActor private static let text: Text = {
        let lines = Theme.bannerLines
        let width = lines.map(\.count).max() ?? 0
        var out = Text("")
        for (y, line) in lines.enumerated() {
            let cells = Array(line.padding(toLength: width, withPad: " ", startingAt: 0))
            for (x, ch) in cells.enumerated() {
                out = out + Text(String(ch))
                    .foregroundColor(Theme.logoColor(x: x, y: y, width: width, height: lines.count))
            }
            if y < lines.count - 1 { out = out + Text("\n") }
        }
        return out
    }()

    var body: some View {
        Logo.text
            .font(.system(size: 11, weight: .bold, design: .monospaced))
            .lineSpacing(0)
            .fixedSize()
            .accessibilityLabel("rival")
    }
}

/// "rival" in the logo gradient, for the breadcrumb (TUI `rivalWord`).
struct RivalWord: View {
    var body: some View {
        let chars = Array("rival")
        chars.enumerated().reduce(Text("")) { acc, pair in
            acc + Text(String(pair.element))
                .foregroundColor(Color(hex: blendHex(Theme.logoStopHex, t: Double(pair.offset) / Double(chars.count - 1))))
        }
        .bold()
    }
}
