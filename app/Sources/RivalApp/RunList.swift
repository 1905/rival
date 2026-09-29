import RivalKit
import SwiftUI

/// The run list: one page of day sections, one row per run. Selection is the
/// run's `RunItem.id`, so it survives refreshes and re-sorts.
///
/// A hand-rolled list rather than `List(selection:)`: the system list paints
/// its own grey or blue selection, which cannot be themed. Here the selected
/// row sits on the accent bar with black text, like the TUI.
///
/// Keys while the list has focus: ↑/↓ move the selection and cross page
/// edges, ←/→ and `[`/`]` turn the page, Home/End go to the first and last
/// run overall.
struct RunList: View {
    /// The sections of the current page only.
    let sections: [RunSection]
    /// Every run the tab and the filter show, across all pages, in list order.
    let ids: [String]
    @Binding var paging: PageState
    @Binding var selection: String?
    @FocusState private var focused: Bool

    private static let topID = "run-list-top"

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0, pinnedViews: [.sectionHeaders]) {
                    Color.clear.frame(height: 0).id(Self.topID)
                    ForEach(sections, id: \.section) { sec in
                        Section {
                            ForEach(sec.items) { item in
                                let selected = selection == item.id
                                RunRow(item: item, selected: selected)
                                    .padding(.horizontal, 10)
                                    .padding(.vertical, 3)
                                    .background(selected ? Theme.selectionBg : Color.clear)
                                    .overlay(alignment: .leading) {
                                        if selected { Rectangle().fill(Theme.accent).frame(width: 2) }
                                    }
                                    .contentShape(Rectangle())
                                    .onTapGesture {
                                        selection = item.id
                                        focused = true
                                    }
                                    .id(item.id)
                            }
                        } header: {
                            Text(sec.section.rawValue)
                                .font(Mono.small)
                                .foregroundStyle(Theme.dim)
                                .padding(.horizontal, 10)
                                .padding(.top, 8)
                                .padding(.bottom, 3)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .background(Theme.background)
                        }
                    }
                }
                .padding(.bottom, 6)
            }
            .background(Theme.background)
            .focusable()
            .focused($focused)
            .focusEffectDisabled()
            .onKeyPress(.downArrow) { select(paging.move(selection: selection, by: 1, in: ids), proxy) }
            .onKeyPress(.upArrow) { select(paging.move(selection: selection, by: -1, in: ids), proxy) }
            .onKeyPress(.rightArrow) { select(paging.turn(by: 1, in: ids), proxy) }
            .onKeyPress(.leftArrow) { select(paging.turn(by: -1, in: ids), proxy) }
            .onKeyPress("]") { select(paging.turn(by: 1, in: ids), proxy) }
            .onKeyPress("[") { select(paging.turn(by: -1, in: ids), proxy) }
            .onKeyPress(.home) { select(paging.first(in: ids), proxy) }
            .onKeyPress(.end) { select(paging.last(in: ids), proxy) }
            // A new page starts at the top, unless the selection landed lower
            // on it (↑ past the top of a page selects the previous page's
            // last row).
            .onChange(of: paging.page) { scrollToSelection(proxy) }
        }
    }

    /// Applies a key's new selection. nil means the key had nothing to do.
    private func select(_ next: String?, _ proxy: ScrollViewProxy) -> KeyPress.Result {
        guard let next else { return .ignored }
        selection = next
        proxy.scrollTo(next)
        return .handled
    }

    private func scrollToSelection(_ proxy: ScrollViewProxy) {
        let first = sections.first?.items.first?.id
        if let sel = selection, sel != first, sections.contains(where: { $0.items.contains { $0.id == sel } }) {
            proxy.scrollTo(sel)
        } else {
            proxy.scrollTo(Self.topID, anchor: .top)
        }
    }
}

/// glyph · kind · model · effort · time · project, in SF Mono.
struct RunRow: View {
    let item: RunItem
    let selected: Bool

    var body: some View {
        let status = runStatus(item)
        let live = status.isLive
        HStack(spacing: 8) {
            StatusGlyph(status: status, selected: selected)
                .frame(width: 14, alignment: .center)
            Text(runKind(item))
                .frame(width: 50, alignment: .leading)
                .foregroundStyle(text(Theme.fg))
            Text(runModelName(item))
                .lineLimit(1)
                .truncationMode(.tail)
                .frame(minWidth: 120, maxWidth: .infinity, alignment: .leading)
                .foregroundStyle(text(Theme.fg))
            Text(runEffort(item))
                .frame(width: 52, alignment: .leading)
                .foregroundStyle(text(Theme.dim))
            Group {
                if live {
                    TimelineView(.periodic(from: .now, by: 1)) { ctx in
                        Text(runTimeLabel(item, now: ctx.date))
                    }
                } else {
                    Text(runTimeLabel(item, now: Date()))
                }
            }
            .frame(width: 76, alignment: .trailing)
            .foregroundStyle(text(Theme.color(for: status)))
            Text(projectName(item.primary?.workDir ?? ""))
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(minWidth: 60, maxWidth: 160, alignment: .leading)
                .foregroundStyle(text(Theme.dim))
        }
        .font(selected ? Mono.bold : Mono.body)
        .padding(.vertical, 1)
        .contentShape(Rectangle())
        .help(item.primary?.promptPreview ?? "")
    }

    /// Selected rows sit on the accent bar and use its black text.
    private func text(_ color: Color) -> Color { selected ? Theme.selectionFg : color }
}

/// The status glyph in its colour; running spins (TUI `statusGlyph`).
struct StatusGlyph: View {
    let status: RunStatus
    var selected = false

    var body: some View {
        Group {
            if status == .running {
                TimelineView(.periodic(from: .now, by: 1.0 / 12)) { ctx in
                    Text(statusGlyph(.running, spin: spinnerFrame(at: ctx.date)))
                }
            } else {
                Text(statusGlyph(status))
            }
        }
        .foregroundStyle(selected ? Theme.selectionFg : Theme.color(for: status))
    }
}
