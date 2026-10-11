import AppKit
import RivalKit
import SwiftUI

/// The status item: "r" plus the live run count, or a plain "r" when idle.
/// Until the first snapshot it shows "…", not a count. The menu
/// bar tints it like every other template icon.
struct MenuBarLabel: View {
    let store: SessionStore

    var body: some View {
        let live = store.runs.lazy.filter { $0.status.isLive }.count
        if store.isLoading {
            // Static on purpose. A TimelineView here re-renders the status item
            // image in a synchronous loop at launch: main thread pinned at 100%
            // CPU, ~40 MB/s leaked, the first scan never publishes, killed at 200 GB.
            Text("\(Image(systemName: "r.square")) …")
                .font(.system(size: 13, weight: .medium, design: .monospaced))
                .accessibilityLabel("Rival, reading sessions")
        } else if live > 0 {
            Text("\(Image(systemName: "r.square.fill")) \(live)")
                .font(.system(size: 13, weight: .medium, design: .monospaced))
                .accessibilityLabel("Rival, \(live) live")
        } else {
            Image(systemName: "r.square")
                .accessibilityLabel("Rival")
        }
    }
}

/// The popover: up to 10 live runs (then "+N more — open Rival"), the last 5
/// finished, the notify toggle, the model check, Settings and Open Rival.
/// Live elapsed times tick once a second; finished ones do not change.
struct MenuBarView: View {
    @Bindable var app: AppModel
    @Environment(\.openWindow) private var openWindow
    @Environment(\.openSettings) private var openSettings
    @AppStorage(notifyOnFinishKey) private var notifyOnFinish = true

    var body: some View {
        let (live, recent) = menuBarRuns(app.store.runs)
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                RivalWord().font(Mono.bold)
                Spacer()
                Text(app.store.isLoading ? "… live" : "\(live.count) live")
                    .font(Mono.small)
                    .foregroundStyle(live.isEmpty ? Theme.dim : Theme.running)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)

            Divider().overlay(Theme.dim)

            VStack(alignment: .leading, spacing: 0) {
                let loading = app.store.isLoading
                if live.isEmpty {
                    block("LIVE", live, empty: loading ? "reading sessions…" : "nothing running", now: Date())
                } else {
                    TimelineView(.periodic(from: .now, by: 1)) { ctx in
                        VStack(alignment: .leading, spacing: 0) {
                            block("LIVE", live, empty: "", now: ctx.date)
                        }
                    }
                }
                block("RECENT", recent, empty: loading ? "reading sessions…" : "no finished runs", now: Date())
            }
            .padding(.bottom, 6)

            Divider().overlay(Theme.dim)

            VStack(alignment: .leading, spacing: 6) {
                Toggle("Notify on finish", isOn: $notifyOnFinish)
                    .toggleStyle(.checkbox)
                    .tint(Theme.accent)
                if notifyOnFinish, let hint = app.notificationHint {
                    Text(hint)
                        .font(Mono.small)
                        .foregroundStyle(Theme.dim)
                        .fixedSize(horizontal: false, vertical: true)
                }
                HStack(spacing: 8) {
                    // The result also arrives as a notification.
                    Button("Check models") { app.settings.runCheck(notify: true) }
                        .disabled(app.settings.checking)
                    checkStatus
                    Spacer()
                    Button("Settings…") { showSettings() }
                        .keyboardShortcut(",")
                }
                HStack {
                    Button("Open Rival") { open(nil) }
                        .keyboardShortcut(.defaultAction)
                    Spacer()
                    Button("Quit") { NSApp.terminate(nil) }
                        .keyboardShortcut("q")
                }
            }
            .padding(12)
        }
        .font(Mono.body)
        .foregroundStyle(Theme.fg)
        .frame(width: 460)
        .background(Theme.background)
        .preferredColorScheme(.dark)
        .onAppear {
            app.openWindowAction = openWindow
            app.openSettingsAction = openSettings
        }
    }

    /// "checking…", "4 of 5 ok" or the error of the last model check.
    @ViewBuilder
    private var checkStatus: some View {
        let settings = app.settings
        if settings.checking {
            Text("checking… \(settings.checkRows.count) done")
                .font(Mono.small)
                .foregroundStyle(Theme.dim)
        } else if let error = settings.checkError {
            Text(error)
                .font(Mono.small)
                .foregroundStyle(Theme.fail)
                .lineLimit(1)
                .truncationMode(.tail)
                .help(error)
        } else if let summary = settings.checkSummary {
            Text(summary.text)
                .font(Mono.small)
                .foregroundStyle(summary.allOK ? Theme.accent : Theme.fail)
        }
    }

    private func showSettings() {
        app.openSettingsAction = openSettings
        NSApp.activate()
        openSettings()
    }

    @ViewBuilder
    private func block(_ title: String, _ runs: [RunItem], empty: String, now: Date) -> some View {
        Text(title)
            .font(Mono.small)
            .foregroundStyle(Theme.dim)
            .padding(.horizontal, 12)
            .padding(.top, 8)
            .padding(.bottom, 3)
        if runs.isEmpty {
            Text(empty)
                .font(Mono.small)
                .foregroundStyle(Theme.dim)
                .padding(.horizontal, 12)
                .padding(.vertical, 2)
        } else {
            // Only LIVE is capped; `menuBarRuns` already limits RECENT to 5.
            let (shown, more) = title == "LIVE" ? capLive(runs) : (runs, 0)
            ForEach(shown) { item in
                MenuRunRow(item: item, now: now) { open(item.id) }
            }
            if more > 0 {
                MoreRow(text: "+\(more) more — open Rival") { open(nil) }
            }
        }
    }

    private func open(_ runID: String?) {
        app.openWindowAction = openWindow
        app.show(runID: runID)
    }
}

/// glyph · kind · model · project · elapsed. Hover lights it; a click opens
/// the run in the main window.
struct MenuRunRow: View {
    let item: RunItem
    let now: Date
    let action: () -> Void
    @State private var hover = false

    var body: some View {
        let status = runStatus(item)
        HStack(spacing: 8) {
            StatusGlyph(status: status, selected: hover)
                .frame(width: 14)
            Text(runKind(item))
                .frame(width: 48, alignment: .leading)
            Text(runModelName(item))
                .lineLimit(1)
                .truncationMode(.tail)
                .frame(maxWidth: .infinity, alignment: .leading)
            Text(projectName(item.primary?.workDir ?? ""))
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(width: 110, alignment: .leading)
                .foregroundStyle(hover ? Theme.selectionFg : Theme.dim)
            Text(runTimeLabel(item, now: now))
                .frame(width: 70, alignment: .trailing)
                .foregroundStyle(hover ? Theme.selectionFg : Theme.color(for: status))
        }
        .font(Mono.body)
        .foregroundStyle(hover ? Theme.selectionFg : Theme.fg)
        .padding(.horizontal, 12)
        .padding(.vertical, 3)
        .background(hover ? Theme.selectionBg : Color.clear)
        .contentShape(Rectangle())
        .onHover { hover = $0 }
        .onTapGesture(perform: action)
        .help(item.primary?.promptPreview ?? "")
    }
}

/// "+N more — open Rival" under a capped LIVE block. A click opens the main
/// window.
struct MoreRow: View {
    let text: String
    let action: () -> Void
    @State private var hover = false

    var body: some View {
        Text(text)
            .font(Mono.small)
            .foregroundStyle(hover ? Theme.selectionFg : Theme.dim)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 12)
            .padding(.vertical, 3)
            .background(hover ? Theme.selectionBg : Color.clear)
            .contentShape(Rectangle())
            .onHover { hover = $0 }
            .onTapGesture(perform: action)
    }
}
