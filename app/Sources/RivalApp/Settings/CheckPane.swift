import RivalKit
import SwiftUI

/// The Check tab: one short live call per model through `rival config check`.
/// Rows appear as each model finishes; the summary line comes last.
struct CheckPane: View {
    @Bindable var settings: SettingsModel

    var body: some View {
        CLIGate(settings: settings) {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 8) {
                    Button("Check models") { settings.runCheck() }
                        .keyboardShortcut(.defaultAction)
                        .disabled(settings.checking)
                    Button("Apply & check") { settings.runCheck(applyFirst: true) }
                        .disabled(settings.checking || !settings.hasDraft)
                        .help("Save the unsaved changes, then check")
                    if settings.checking {
                        ProgressView().controlSize(.small)
                        Button("Stop") { settings.cancelCheck() }
                            .buttonStyle(.borderless)
                    }
                    Spacer()
                    CheckSummaryLabel(settings: settings)
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 12)

                if let summary = settings.checkSummary {
                    CheckProxyLine(proxy: summary.proxy)
                        .padding(.horizontal, 20)
                        .padding(.bottom, 8)
                }

                Table(settings.checkRows) {
                    TableColumn("") { row in
                        CheckIcon(state: row.state)
                    }
                    .width(20)
                    TableColumn("Model") { row in
                        Text(row.name)
                    }
                    .width(min: 60, ideal: 70)
                    TableColumn("Route") { row in
                        Text(row.route).foregroundStyle(.secondary)
                    }
                    .width(min: 50, ideal: 55)
                    TableColumn("Wire id") { row in
                        Text(row.wireModel)
                            .font(.system(.body, design: .monospaced))
                            .textSelection(.enabled)
                    }
                    .width(min: 140, ideal: 190)
                    TableColumn("Time") { row in
                        Text(row.latency)
                            .font(.system(.body, design: .monospaced))
                            .foregroundStyle(.secondary)
                    }
                    .width(min: 45, ideal: 50)
                    TableColumn("Result") { row in
                        CheckMessage(row: row)
                    }
                }
                .overlay {
                    if settings.checkRows.isEmpty {
                        CheckPlaceholder(settings: settings)
                    }
                }
            }
        }
    }
}

/// ✓ green, ⚠ yellow (answered, but not `ok`), ✗ orange (limit), ✗ red.
struct CheckIcon: View {
    let state: CheckState

    var body: some View {
        switch state {
        case .ok:
            Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
                .accessibilityLabel("ok")
        case .unexpected:
            Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.yellow)
                .accessibilityLabel("ok, unexpected reply")
        case .limit:
            Image(systemName: "xmark.octagon.fill").foregroundStyle(.orange)
                .accessibilityLabel("account limit")
        case .failed:
            Image(systemName: "xmark.octagon.fill").foregroundStyle(.red)
                .accessibilityLabel("failed")
        }
    }
}

/// The reply or the error; the hint under it. Full text on hover.
struct CheckMessage: View {
    let row: CheckRow

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            HStack(spacing: 4) {
                if row.state == .limit {
                    Text("limit")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(.orange)
                }
                Text(row.message)
                    .foregroundStyle(row.state == .unexpected ? Color.yellow : row.ok ? Color.primary : Color.red)
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
            if !row.ok, !row.hint.isEmpty {
                Text(row.hint)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
        }
        .help(row.hint.isEmpty ? row.message : row.message + "\n" + row.hint)
    }
}

/// "4 of 5 ok · 14:02", "checking… 2 done", or the error.
struct CheckSummaryLabel: View {
    @Bindable var settings: SettingsModel

    var body: some View {
        if settings.checking {
            Text("checking… \(settings.checkRows.count) done")
                .foregroundStyle(.secondary)
        } else if let error = settings.checkError {
            Label(error, systemImage: "exclamationmark.triangle.fill")
                .foregroundStyle(.red)
                .lineLimit(1)
                .help(error)
        } else if let summary = settings.checkSummary {
            HStack(spacing: 4) {
                Text(summary.text)
                    .fontWeight(.semibold)
                    .foregroundStyle(summary.allOK ? Color.green : Color.red)
                if let at = settings.checkFinished {
                    Text("· \(at.formatted(date: .omitted, time: .shortened))")
                        .foregroundStyle(.secondary)
                }
            }
        }
    }
}

/// proxy  http://127.0.0.1:8317  up · 38 models · key …a91f
struct CheckProxyLine: View {
    let proxy: CheckProxy

    var body: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(proxy.state == "up" ? Color.green : proxy.state == "down" ? Color.red : Color.secondary)
                .frame(width: 8, height: 8)
            Text(text)
                .font(.system(.callout, design: .monospaced))
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
                .help(text)
        }
    }

    private var text: String {
        switch proxy.state {
        case "up":
            let key = proxy.keyTail.isEmpty ? "" : " · key …\(proxy.keyTail)"
            return "proxy \(proxy.url) · \(proxy.models) models\(key)"
        case "down":
            return "proxy \(proxy.url) · \(proxy.error)"
        default:
            return "proxy off · every model runs direct"
        }
    }
}

struct CheckPlaceholder: View {
    @Bindable var settings: SettingsModel

    var body: some View {
        if settings.checking {
            ProgressView("Sending “Reply with exactly: ok” to each model…")
        } else {
            VStack(spacing: 6) {
                Text("No check yet")
                    .font(.headline)
                Text("Check models sends one short prompt to each model, the way a review calls it.")
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
            }
            .frame(maxWidth: 360)
        }
    }
}
