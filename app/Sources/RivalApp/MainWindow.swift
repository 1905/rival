import RivalKit
import SwiftUI

/// Monospaced text in the TUI's sizes. SF Mono is the system monospaced face.
enum Mono {
    static let body = Font.system(size: 12, design: .monospaced)
    static let small = Font.system(size: 11, design: .monospaced)
    static let bold = Font.system(size: 12, weight: .semibold, design: .monospaced)
}

struct MainWindow: View {
    @Bindable var app: AppModel
    @State private var tab: StatusTab = .all
    @State private var search = ""
    @State private var toast: String?
    @Environment(\.openWindow) private var openWindow

    private var store: SessionStore { app.store }

    var body: some View {
        NavigationSplitView {
            Sidebar(app: app, tab: $tab, search: search)
                .searchable(text: $search, placement: .sidebar, prompt: "filter: model, project, status…")
                .navigationSplitViewColumnWidth(min: 420, ideal: 520, max: 800)
        } detail: {
            Group {
                if let item = app.selectedRun {
                    RunDetail(item: item, store: store, toast: $toast)
                } else {
                    EmptyState(title: "No run selected", detail: "Pick a run on the left.")
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Theme.background)
        }
        .overlay(alignment: .bottom) { ToastView(text: $toast) }
        .background(Theme.background)
        .foregroundStyle(Theme.fg)
        .tint(Theme.accent)
        .preferredColorScheme(.dark)
        .toolbarBackground(Theme.background, for: .windowToolbar)
        .frame(minWidth: 900, minHeight: 520)
        // The notifier and the dock need a way to reopen this window after
        // it was closed; the action is only reachable from a view.
        .onAppear { app.openWindowAction = openWindow }
    }
}

// MARK: - Sidebar

struct Sidebar: View {
    @Bindable var app: AppModel
    @Binding var tab: StatusTab
    let search: String
    /// The current page of the run list. Lives here, not in `RunList`, so it
    /// survives the list being swapped for an empty state and back.
    @State private var paging = PageState()

    var body: some View {
        let store = app.store
        let (sections, counts) = visibleSections()
        let ids = sections.flatMap { $0.items.map(\.id) }
        VStack(alignment: .leading, spacing: 0) {
            SidebarHeader(stats: store.stats, loading: store.isLoading)
                .padding(.horizontal, 12)
                .padding(.vertical, 10)

            Segmented(selection: $tab, options: StatusTab.allCases, fill: true) { t in
                store.isLoading ? "\(t.label) …" : "\(t.label) \(counts[t] ?? 0)"
            }
            .padding(.horizontal, 12)
            .padding(.bottom, 8)

            Divider().overlay(Theme.dim)

            if store.isLoading {
                SessionLoader(progress: store.loadProgress)
            } else if !store.directoryExists {
                EmptyState(
                    title: "No runs yet",
                    detail: "Run a review with rival.\nWatching \(store.sessionsDir.path)"
                )
            } else if sections.isEmpty {
                EmptyState(
                    title: store.runs.isEmpty ? "No runs yet" : "No runs match",
                    detail: store.runs.isEmpty ? "Run a review with rival." : "Change the filter or the status tab."
                )
            } else {
                RunList(
                    sections: pageSections(sections, page: paging.page, size: paging.size),
                    ids: ids,
                    paging: $paging,
                    selection: $app.selectedRunID
                )
                Divider().overlay(Theme.dim)
                PageFooterBar(footer: paging.footer(total: ids.count)) { delta in
                    if let first = paging.turn(by: delta, in: ids) { app.selectedRunID = first }
                }
            }
        }
        .background(Theme.background)
        // A refresh or an outside selection (menu bar, notification) shows the
        // page of the selected run. A vanished run passes the selection to
        // the run at its old index, clamped.
        .onChange(of: ids) { old, new in
            if let next = paging.refresh(selection: app.selectedRunID, from: old, to: new) {
                app.selectedRunID = next
            }
        }
        .onChange(of: app.selectedRunID) { _, sel in paging.anchor(selection: sel, in: ids) }
        .onChange(of: tab) { resetPage(ids) }
        .onChange(of: search) { resetPage(ids) }
        .onAppear { paging.anchor(selection: app.selectedRunID, in: ids) }
    }

    private func visibleSections() -> (sections: [RunSection], counts: [StatusTab: Int]) {
        runSections(app.store.runs, tab: tab, terms: filterTerms(search), now: Date(), calendar: .current)
    }

    /// A tab or filter change: page 1, the cursor on its first run. `ids` is
    /// the list the body just built for the new tab and filter.
    private func resetPage(_ ids: [String]) {
        paging.reset()
        if let first = ids.first { app.selectedRunID = first }
    }
}

/// `‹ prev  page P/N  next ›  · T runs` under the list, or just `T runs` on a
/// single page. The arrows click; an unavailable one is dimmed.
struct PageFooterBar: View {
    let footer: PageFooter
    let turn: (Int) -> Void

    var body: some View {
        HStack(spacing: 0) {
            if footer.paged {
                arrow("‹ prev", enabled: footer.hasPrev) { turn(-1) }
                Text("  \(footer.pageLabel)  ").foregroundStyle(Theme.fg)
                arrow("next ›", enabled: footer.hasNext) { turn(1) }
                Text("  · \(footer.runsLabel)").foregroundStyle(Theme.dim)
            } else {
                Text(footer.runsLabel).foregroundStyle(Theme.dim)
            }
            Spacer(minLength: 0)
        }
        .font(Mono.small)
        .lineLimit(1)
        .padding(.horizontal, 12)
        .padding(.vertical, 5)
        .background(Theme.background)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(footer.text)
    }

    private func arrow(_ label: String, enabled: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(label)
                .foregroundStyle(enabled ? Theme.accent : Theme.dim)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
    }
}

/// The logo with the live session counts beside it (TUI `renderHeader`).
/// While `loading`, every count is "…" (TUI: the same).
struct SidebarHeader: View {
    let stats: SessionStats
    var loading = false

    var body: some View {
        HStack(alignment: .center, spacing: 16) {
            Logo()
            Spacer(minLength: 0)
            VStack(alignment: .trailing, spacing: 3) {
                HStack(spacing: 10) {
                    SpinnerText(live: loading || stats.running > 0, idle: "●") { "\($0) \(n(stats.running)) running" }
                        .foregroundStyle(Theme.running)
                    Text("◌ \(n(stats.queued)) queued").foregroundStyle(Theme.queued)
                }
                HStack(spacing: 10) {
                    Text("✓ \(n(stats.completed))").foregroundStyle(Theme.accent)
                    Text("✗ \(n(stats.failed))").foregroundStyle(Theme.fail)
                }
                Text("\(n(stats.total)) sessions").foregroundStyle(Theme.dim)
            }
            .font(Mono.small)
        }
    }
}

extension SidebarHeader {
    private func n(_ v: Int) -> String { loading ? "…" : "\(v)" }
}

/// The startup loader in the list area until the first snapshot: a spinner
/// and "reading sessions done/total" over a bar in the logo gradient (TUI
/// `loaderView`).
struct SessionLoader: View {
    let progress: LoadProgress

    private var fraction: Double {
        progress.total > 0 ? min(1, Double(progress.done) / Double(progress.total)) : 0
    }

    private var label: String {
        progress.total > 0 ? "reading sessions \(progress.done)/\(progress.total)" : "reading sessions"
    }

    var body: some View {
        VStack(spacing: 10) {
            HStack(spacing: 6) {
                SpinnerText(live: true) { $0 }.foregroundStyle(Theme.running)
                Text(label).foregroundStyle(Theme.fg).monospacedDigit()
            }
            .font(Mono.small)
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule().fill(Theme.dim.opacity(0.35))
                    // The gradient spans the whole track and is revealed by
                    // the fill, like the TUI bar.
                    LinearGradient(colors: Theme.logoStops, startPoint: .leading, endPoint: .trailing)
                        .mask(alignment: .leading) {
                            Capsule().frame(width: geo.size.width * fraction)
                        }
                }
            }
            .frame(width: 240, height: 4)
            .animation(.easeOut(duration: 0.2), value: fraction)
        }
        .padding(24)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(label)
    }
}

/// Text built from the current spinner frame, animated only while `live`.
struct SpinnerText: View {
    let live: Bool
    var idle = "⠋"
    let make: (String) -> String

    var body: some View {
        if live {
            TimelineView(.periodic(from: .now, by: 1.0 / 12)) { ctx in
                Text(make(spinnerFrame(at: ctx.date)))
            }
        } else {
            Text(make(idle))
        }
    }
}

struct EmptyState: View {
    let title: String
    let detail: String

    var body: some View {
        VStack(spacing: 8) {
            Text(title).font(Mono.bold).foregroundStyle(Theme.fg)
            Text(detail).font(Mono.small).foregroundStyle(Theme.dim).multilineTextAlignment(.center)
        }
        .padding(24)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

/// A one-line result at the bottom of the window. Clears itself after 4 s.
struct ToastView: View {
    @Binding var text: String?

    var body: some View {
        if let t = text {
            Text(t)
                .font(Mono.body)
                .foregroundStyle(Theme.fg)
                .padding(.horizontal, 14)
                .padding(.vertical, 8)
                .background(Theme.background, in: RoundedRectangle(cornerRadius: 6))
                .overlay(RoundedRectangle(cornerRadius: 6).stroke(Theme.accent, lineWidth: 1))
                .padding(.bottom, 18)
                .transition(.opacity)
                .task(id: t) {
                    try? await Task.sleep(for: .seconds(4))
                    if text == t { withAnimation { text = nil } }
                }
        }
    }
}
