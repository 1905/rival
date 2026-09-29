import SwiftUI

/// The app's dim-phosphor palette. Same token names as the TUI's
/// `rival/internal/dashboard/styles.go`, but softer values: the TUI's neon
/// greens on a large window were too bright (user, 2026-09-29). Colour marks
/// state only: running amber, failed red; completed stays quiet.
public enum Theme {
    /// Body text.
    public static let fg = Color(hex: 0xB4C2B7)
    /// Secondary text, borders, help.
    public static let dim = Color(hex: 0x4F6154)
    /// Active marks: selection bar edge, tab text, focus border, links.
    public static let accent = Color(hex: 0x6FC985)
    /// Amber: spinner, running status.
    public static let running = Color(hex: 0xD6A34E)
    /// Grey-green: waiting in line.
    public static let queued = Color(hex: 0x6A7F70)
    /// Finished fine, in lists and headers: quiet, because it is the normal
    /// case. Only running and failed get a colour.
    public static let done = Color(hex: 0x7F9483)
    /// Failed.
    public static let fail = Color(hex: 0xDB6B6B)
    /// Fill behind the selected row, the active tab and hovered menu rows.
    public static let selectionBg = Color(hex: 0x16241A)
    /// Text on `selectionBg`.
    public static let selectionFg = Color(hex: 0xE2EDE4)
    /// Window background. Near-black, not pure black: easier on the eyes
    /// next to light text.
    public static let background = Color(hex: 0x0A0E0B)

    /// ASCII-logo gradient stops: violet → cyan → green.
    public static let logoStopHex: [UInt32] = [0x8B6CD9, 0x4FB8CC, 0x6FC985]
    public static let logoStops: [Color] = logoStopHex.map { Color(hex: $0) }

    /// The logo colour of the cell at column `x`, row `y` of a `width`×`height`
    /// block: a 45° gradient over `logoStopHex`, like the TUI's `Blend2D`.
    public static func logoColor(x: Int, y: Int, width: Int, height: Int) -> Color {
        let fx = width > 1 ? Double(x) / Double(width - 1) : 0
        let fy = height > 1 ? Double(y) / Double(height - 1) : 0
        return Color(hex: blendHex(logoStopHex, t: (fx + fy) / 2))
    }

    /// The ASCII logo (`bannerLines`).
    public static let bannerLines: [String] = [
        #"         _             __"#,
        #"   _____(_)   ______ _/ /"#,
        #"  / ___/ / | / / __ `/ /"#,
        #" / /  / /| |/ / /_/ / /"#,
        #"/_/  /_/ |___/\__,_/_/"#,
    ]

    /// Status colour (`statusStyle`); an unknown status uses body text.
    public static func color(for status: RunStatus) -> Color {
        switch status {
        case .running: return running
        case .queued: return queued
        case .completed: return done
        case .failed: return fail
        case .unknown: return fg
        }
    }
}

/// Linear sRGB blend across evenly spaced `stops` at `t` in 0...1 (clamped).
public func blendHex(_ stops: [UInt32], t: Double) -> UInt32 {
    guard let first = stops.first else { return 0 }
    guard stops.count > 1 else { return first }
    let pos = min(max(t, 0), 1) * Double(stops.count - 1)
    let i = min(Int(pos), stops.count - 2)
    let f = pos - Double(i)
    func ch(_ c: UInt32, _ shift: UInt32) -> Double { Double((c >> shift) & 0xFF) }
    func mix(_ shift: UInt32) -> UInt32 {
        UInt32((ch(stops[i], shift) * (1 - f) + ch(stops[i + 1], shift) * f).rounded()) << shift
    }
    return mix(16) | mix(8) | mix(0)
}

extension Color {
    /// An sRGB colour from 0xRRGGBB.
    public init(hex: UInt32) {
        self.init(
            .sRGB,
            red: Double((hex >> 16) & 0xFF) / 255,
            green: Double((hex >> 8) & 0xFF) / 255,
            blue: Double(hex & 0xFF) / 255,
            opacity: 1
        )
    }
}
