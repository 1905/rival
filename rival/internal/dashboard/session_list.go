package dashboard

import (
	"fmt"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"charm.land/bubbles/v2/textinput"
	"charm.land/lipgloss/v2"
	"github.com/1905/rival/internal/session"
	"github.com/1905/rival/internal/sessionview"
	"github.com/charmbracelet/x/ansi"
)

// The derivations below delegate to internal/sessionview, the single source
// of the grouping rules. Only presentation stays here.

func groupEffort(item *displayItem) string {
	return sessionview.Effort(item.Sessions)
}

func groupStatus(item *displayItem) string {
	return sessionview.Status(item.Sessions)
}

// groupElapsed is the wall-clock span of the whole group. It previously
// reported the longest single member instead of the whole span.
func groupElapsed(item *displayItem) string {
	return sessionview.Elapsed(item.Sessions)
}

func formatElapsed(s *session.Session) string {
	if s.Duration != "" {
		return s.Duration
	}
	if s.Status == "running" {
		d := time.Since(s.StartTime).Round(time.Second)
		return d.String()
	}
	// Queued: show how long it has been waiting in line.
	if s.Status == "queued" && s.QueuedAt != nil {
		return time.Since(*s.QueuedAt).Round(time.Second).String()
	}
	return "-"
}

// statusTab is the status filter above the list.
type statusTab int

const (
	tabAll statusTab = iota
	tabRunning
	tabFailed
	tabDone
)

// statusTabCount is the number of statusTab values, for cycling and counts.
const statusTabCount = 4

func (t statusTab) label() string {
	switch t {
	case tabRunning:
		return "RUNNING"
	case tabFailed:
		return "FAILED"
	case tabDone:
		return "DONE"
	default:
		return "ALL"
	}
}

// accepts reports whether a row with this status belongs on tab t. Queued runs
// sit under RUNNING: both are "not finished yet" from the user's side.
func (t statusTab) accepts(status string) bool {
	switch t {
	case tabRunning:
		return isLive(status)
	case tabFailed:
		return status == "failed"
	case tabDone:
		return status == "completed"
	default:
		return true
	}
}

// row is one line of the list body. Section != "" marks a non-selectable
// header row (TODAY, YESTERDAY, ...); otherwise Item is the run.
type row struct {
	Section string
	Item    *displayItem
}

// modelName is the model id the TUI shows: the stored id verbatim, so
// "gpt-6-astra" and "claude-fable-5" stay readable instead of collapsing to a
// public label or "retired-model". The CLI stands in when no model was
// recorded.
func modelName(s *session.Session) string {
	if s.Model != "" {
		return s.Model
	}
	return s.CLI
}

// groupModelName is the MODEL cell for a row: the first member's model plus
// "+N" for the other distinct models. The judge usually reuses a reviewer's
// model, so counting distinct ids keeps "+N" honest.
func groupModelName(item *displayItem) string {
	first := item.Primary()
	if first == nil {
		return ""
	}
	name := modelName(first)
	seen := map[string]bool{name: true}
	for _, s := range item.Sessions[1:] {
		seen[modelName(s)] = true
	}
	if n := len(seen) - 1; n > 0 {
		return fmt.Sprintf("%s +%d", name, n)
	}
	return name
}

// kindLabel is the KIND cell: what the run does, never how it was
// transported. A group classifies through sessionview; a solo run maps its
// own Mode.
func kindLabel(item *displayItem) string {
	if item.IsGroup() {
		return shortKind(sessionview.Kind(item.Sessions))
	}
	s := item.Primary()
	if s == nil {
		return ""
	}
	kind := shortKind(s.Mode)
	if s.Mode == "docker" && (s.CLI == "claude" || s.CLI == "fable") {
		// "fable" is read-compat for sessions written before 3.34.
		kind += "/dk"
	}
	return kind
}

// shortKind maps a session mode or sessionview kind to its column label.
// Transport modes (native, docker) and unset modes are plain reviews.
func shortKind(mode string) string {
	switch mode {
	case "megareview", "consilium":
		return "mega"
	case session.ModeSecurity:
		return "sec"
	case session.ModeAntislop:
		return "slop"
	case session.ModePlan:
		return "plan"
	case "raw":
		return "raw"
	case "", "review", "native", "docker":
		return "review"
	default:
		return mode
	}
}

// projectName is the last element of the work dir: the part that tells runs
// apart, which a truncated absolute path cuts off.
func projectName(workdir string) string {
	if workdir == "" {
		return "-"
	}
	return filepath.Base(workdir)
}

// Section labels, in display order.
var sectionOrder = []string{"TODAY", "YESTERDAY", "THIS WEEK", "OLDER"}

// dayBounds are the section boundaries for one "now": the starts of today,
// yesterday and THIS WEEK (the five days before yesterday). Day starts come
// from time.Date so a DST switch cannot shift a boundary by an hour.
type dayBounds struct{ today, yesterday, week time.Time }

func newDayBounds(now time.Time) dayBounds {
	y, m, d := now.Date()
	loc := now.Location()
	return dayBounds{
		today:     time.Date(y, m, d, 0, 0, 0, 0, loc),
		yesterday: time.Date(y, m, d-1, 0, 0, 0, 0, loc),
		week:      time.Date(y, m, d-6, 0, 0, 0, 0, loc),
	}
}

// sectionIndex is t's index in sectionOrder.
func (b dayBounds) sectionIndex(t time.Time) int {
	switch {
	case t.IsZero():
		return 3
	case !t.Before(b.today):
		return 0
	case !t.Before(b.yesterday):
		return 1
	case !t.Before(b.week):
		return 2
	default:
		return 3
	}
}

// sectionFor buckets t relative to now by local calendar day.
func sectionFor(t, now time.Time) string {
	return sectionOrder[newDayBounds(now).sectionIndex(t)]
}

// itemTime is when a run appeared: its start, or its queue time when it has
// not started yet.
func itemTime(item *displayItem) time.Time {
	s := item.Primary()
	if s == nil {
		return time.Time{}
	}
	if s.StartTime.IsZero() && s.QueuedAt != nil {
		return *s.QueuedAt
	}
	return s.StartTime
}

// itemStatus is the row status: a solo run's own status, a group's reduced
// status.
func itemStatus(item *displayItem) string {
	if item.IsGroup() {
		return groupStatus(item)
	}
	if s := item.Primary(); s != nil {
		return s.Status
	}
	return ""
}

// filterTerms splits a filter into lowercase, AND-ed terms.
func filterTerms(filter string) []string {
	return strings.Fields(strings.ToLower(filter))
}

// matchesFilter reports whether every term (already lowercase) occurs in the
// item's searchable text: status, kind, model, effort, project, prompt
// preview, review scope and short id of every member.
func matchesFilter(item *displayItem, terms []string) bool {
	if len(terms) == 0 {
		return true
	}
	if item.hay == "" {
		item.hay = strings.ToLower(filterHaystack(item))
	}
	for _, t := range terms {
		if !strings.Contains(item.hay, t) {
			return false
		}
	}
	return true
}

// filterHaystack joins the searchable fields with a separator no term can
// contain (terms are split on whitespace), so a match never spans two fields.
func filterHaystack(item *displayItem) string {
	var b strings.Builder
	b.Grow(256)
	b.WriteString(itemStatus(item))
	b.WriteByte('\n')
	b.WriteString(kindLabel(item))
	for _, s := range item.Sessions {
		for _, f := range [...]string{s.Status, modelName(s), s.Effort, projectName(s.WorkDir), s.PromptPreview, s.ReviewScope, shortID(s.ID)} {
			b.WriteByte('\n')
			b.WriteString(f)
		}
	}
	return b.String()
}

func shortID(id string) string {
	if len(id) > 8 {
		return id[:8]
	}
	return id
}

// rowsAndCounts applies the status tab and the text filter, then groups what
// is left under section headers. Empty sections are omitted. Items keep their
// relative order inside a section, so the watcher's newest-first sort holds.
// It also returns the per-tab counts of filter matches, from the same pass,
// so the tab bar costs nothing extra per keystroke.
func rowsAndCounts(items []displayItem, tab statusTab, terms []string, now time.Time) ([]row, [statusTabCount]int) {
	var counts [statusTabCount]int
	bounds := newDayBounds(now)
	buckets := make([][]*displayItem, len(sectionOrder))
	for i := range items {
		item := &items[i]
		if !matchesFilter(item, terms) {
			continue
		}
		status := itemStatus(item)
		for t := statusTab(0); t < statusTabCount; t++ {
			if t.accepts(status) {
				counts[t]++
			}
		}
		if !tab.accepts(status) {
			continue
		}
		si := bounds.sectionIndex(itemTime(item))
		buckets[si] = append(buckets[si], item)
	}
	var rows []row
	for si, members := range buckets {
		if len(members) == 0 {
			continue
		}
		rows = append(rows, row{Section: sectionOrder[si]})
		for _, item := range members {
			rows = append(rows, row{Item: item})
		}
	}
	return rows, counts
}

// statusGlyph gives each status its own shape, so status reads without colour.
// Running rows show the live spinner frame.
func statusGlyph(status, spin string) string {
	switch status {
	case "running":
		return spin
	case "queued":
		return "◌"
	case "completed":
		return "✓"
	case "failed":
		return "✗"
	default:
		return "·"
	}
}

// fitCell truncates s to w display cells and pads it to exactly w. It measures
// cells, not bytes or runes: fmt's %-*s counts bytes, so every 3-byte glyph
// used to shift the columns after it.
func fitCell(s string, w int) string {
	if w <= 0 {
		return ""
	}
	s = ansi.Truncate(s, w, "…")
	if pad := w - ansi.StringWidth(s); pad > 0 {
		s += strings.Repeat(" ", pad)
	}
	return s
}

// columns are the list column widths in cells. A zero width drops the column.
type columns struct{ Status, Kind, Model, Effort, Time, Project int }

// Narrow-pane thresholds: EFF drops first, then PROJECT. They are the fixed
// column sums (leading space + cells + gaps), so a column only drops when it
// cannot fit at all. There is no prompt column: every review prompt starts
// with the same boilerplate, so it was noise; PROJECT takes the spare width.
const (
	effortMinWidth  = 68 // 1 + (3+1)+(8+1)+(20+1)+(7+1)+(7+1)+(16+1)
	projectMinWidth = 60 // same minus the EFF column
	listFixedWidth  = effortMinWidth
)

// layoutColumns sizes the columns for a pane width. The row is a leading space
// then the cells separated by single spaces; PROJECT takes whatever is left.
func layoutColumns(width int) columns {
	c := columns{Status: 3, Kind: 8, Model: 20, Time: 7}
	if width >= effortMinWidth {
		c.Effort = 7
	}
	if width >= projectMinWidth {
		c.Project = 16
	}
	used := 1 // leading space
	for _, w := range []int{c.Status, c.Kind, c.Model, c.Effort, c.Time, c.Project} {
		if w > 0 {
			used += w + 1
		}
	}
	// used counts a gap after the last cell too, so the row is used-1 wide.
	if c.Project > 0 {
		c.Project += max(0, width-(used-1))
	}
	return c
}

// joinCells lays cells out per the column widths, dropping zero-width columns.
// styles, when non-nil, colours each fitted cell; padding is applied first, so
// styling cannot change the width.
func joinCells(c columns, cells [6]string, styles *[6]lipgloss.Style) string {
	widths := [6]int{c.Status, c.Kind, c.Model, c.Effort, c.Time, c.Project}
	var b strings.Builder
	b.WriteByte(' ')
	first := true
	for i, w := range widths {
		if w <= 0 {
			continue
		}
		if !first {
			b.WriteByte(' ')
		}
		first = false
		cell := fitCell(cells[i], w)
		if styles != nil {
			cell = styles[i].Render(cell)
		}
		b.WriteString(cell)
	}
	return b.String()
}

// oneLine flattens text (a review scope) so a newline or tab cannot break a line.
func oneLine(s string) string {
	return strings.Join(strings.Fields(s), " ")
}

// rowTime is the TIME cell: the elapsed span, prefixed with the queue position
// for a queued solo run.
func rowTime(item *displayItem) string {
	if item.IsGroup() {
		return groupElapsed(item)
	}
	s := item.Primary()
	t := formatElapsed(s)
	if s.Status == "queued" && s.QueuePosition > 0 {
		t = fmt.Sprintf("#%d %s", s.QueuePosition, t)
	}
	return t
}

// renderRow renders one run as exactly width cells.
func renderRow(item *displayItem, c columns, width int, spin string, selected bool) string {
	s := item.Primary()
	status := itemStatus(item)
	cells := [6]string{
		statusGlyph(status, spin),
		kindLabel(item),
		groupModelName(item),
		groupEffort(item),
		rowTime(item),
		projectName(s.WorkDir),
	}
	if selected {
		// One style over plain text: per-cell styles would reset the bar's
		// background mid-row.
		return selectedStyle.Render(fitCell(joinCells(c, cells, nil), width))
	}
	styles := [6]lipgloss.Style{statusStyle(status), textStyle, textStyle, dimStyle, textStyle, dimStyle}
	return fitCell(joinCells(c, cells, &styles), width)
}

// listPane is the run list: status tabs, filter input, sectioned rows and
// the cursor.
type listPane struct {
	// rows is every row that passes the tab and the filter, across all
	// pages. cursor indexes it; offset scrolls inside the current page.
	rows           []row
	cursor, offset int
	// runRows is the row index of each run, in order: runRows[i] is the i-th
	// run. It maps a run ordinal to its row and, by binary search, back.
	runRows []int
	tab     statusTab
	filter  textinput.Model
	// items is the full, unfiltered set the rows are derived from; tab and
	// filter changes rebuild rows from it without waiting for a new event.
	items []displayItem
	// counts are the filter matches per tab, for the tab bar.
	counts [statusTabCount]int
	// stats are the header's per-status session counts and anyLive says
	// whether anything runs or waits. countStatuses derives both once per
	// data change, so frames and ticks never rescan every session.
	stats   headerStats
	anyLive bool
}

func newListPane() listPane {
	return listPane{filter: newFilterInput()}
}

// newFilterInput builds the "/" filter input in the phosphor palette.
func newFilterInput() textinput.Model {
	ti := textinput.New()
	ti.Prompt = "/ "
	ti.Placeholder = "filter"
	ti.CharLimit = 200
	ti.SetWidth(24)
	st := ti.Styles()
	st.Focused.Prompt = lipgloss.NewStyle().Foreground(colAccent)
	st.Focused.Text = textStyle
	st.Focused.Placeholder = dimStyle
	st.Blurred.Prompt = dimStyle
	st.Blurred.Text = textStyle
	st.Blurred.Placeholder = dimStyle
	st.Cursor.Color = colAccent
	ti.SetStyles(st)
	return ti
}

// setItems replaces the data and keeps the cursor on the same run. The index
// alone is not a stable handle: a queued run starting re-sorts the list, and
// following the index would silently swap the selected run.
func (l *listPane) setItems(items []displayItem, now time.Time) {
	anchor := l.selectedKey()
	l.items = items
	l.countStatuses()
	l.rebuild(now, anchor)
}

// countStatuses recounts sessions (not rows) per status. Call it whenever a
// session's status changes in place.
func (l *listPane) countStatuses() {
	var st headerStats
	for _, item := range l.items {
		st.Total += len(item.Sessions)
		for _, s := range item.Sessions {
			switch s.Status {
			case "running":
				st.Running++
			case "queued":
				st.Queued++
			case "completed":
				st.Completed++
			case "failed":
				st.Failed++
			}
		}
	}
	l.stats = st
	l.anyLive = st.Running+st.Queued > 0
}

// rebuild re-derives rows and counts from items, tab and filter, then puts the
// cursor back on anchor. It reports whether anchor survived; when it did not,
// the cursor clamps to the nearest run.
func (l *listPane) rebuild(now time.Time, anchor string) bool {
	l.rows, l.counts = rowsAndCounts(l.items, l.tab, filterTerms(l.filter.Value()), now)
	l.runRows = l.runRows[:0]
	for i, r := range l.rows {
		if r.Item != nil {
			l.runRows = append(l.runRows, i)
		}
	}
	if kind, id, ok := strings.Cut(anchor, ":"); ok {
		group := kind == "group"
		for i, r := range l.rows {
			if r.Item == nil {
				continue
			}
			// itemKey without building a string per row.
			if s := r.Item.Primary(); s != nil && (group && s.GroupID == id || !group && s.GroupID == "" && s.ID == id) {
				l.cursor = i
				return true
			}
		}
	}
	l.cursor = min(l.cursor, len(l.rows)-1)
	if l.cursor < 0 {
		l.cursor = 0
	}
	if l.cursor < len(l.rows) && l.rows[l.cursor].Item == nil {
		// Landed on a section header: prefer the run below it.
		if !l.step(1) {
			l.step(-1)
		}
	}
	return false
}

// step moves the cursor to the next run in direction dir (±1), skipping
// section rows. It reports whether a run was found.
func (l *listPane) step(dir int) bool {
	for i := l.cursor + dir; i >= 0 && i < len(l.rows); i += dir {
		if l.rows[i].Item != nil {
			l.cursor = i
			return true
		}
	}
	return false
}

// move moves the cursor delta runs, skipping section rows; it stops at the
// ends.
func (l *listPane) move(delta int) {
	dir := 1
	if delta < 0 {
		dir, delta = -1, -delta
	}
	for ; delta > 0; delta-- {
		if !l.step(dir) {
			return
		}
	}
}

// top puts the cursor on the first run.
func (l *listPane) top() {
	l.cursor = -1
	if !l.step(1) {
		l.cursor = 0
	}
	l.offset = 0
}

// bottom puts the cursor on the last run.
func (l *listPane) bottom() {
	l.cursor = len(l.rows)
	if !l.step(-1) {
		l.cursor = 0
	}
}

// selectedKey is itemKey of the selected run, or "" when nothing is selected.
func (l *listPane) selectedKey() string {
	if sel := l.selected(); sel != nil {
		return itemKey(sel)
	}
	return ""
}

// selected returns the run under the cursor, or nil.
func (l *listPane) selected() *displayItem {
	if l.cursor < 0 || l.cursor >= len(l.rows) {
		return nil
	}
	return l.rows[l.cursor].Item
}

// cycleTab switches the status tab by delta. Like a filter change, it goes
// back to page 1 with the cursor on the first run.
func (l *listPane) cycleTab(delta int, now time.Time) {
	l.tab = statusTab((int(l.tab) + delta + statusTabCount) % statusTabCount)
	l.refilter(now)
}

// refilter rebuilds after the tab or the filter text changed and resets to
// page 1 with the cursor on the first run. The old selection may sit on any
// page of the new result, so keeping it would leave the user mid-list.
func (l *listPane) refilter(now time.Time) {
	l.rebuild(now, "")
	l.top()
}

// pageSize is how many runs one page holds. Section headers do not count.
const pageSize = 50

// pageCount is the number of pages, at least 1 even when the list is empty.
func (l *listPane) pageCount() int {
	return max(1, (len(l.runRows)+pageSize-1)/pageSize)
}

// page is the 0-based page that holds the cursor. The page is derived from
// the cursor, never stored, so anchoring the cursor by run id after a
// refresh also picks the page that holds the run.
func (l *listPane) page() int {
	if len(l.runRows) == 0 {
		return 0
	}
	i := min(sort.SearchInts(l.runRows, l.cursor), len(l.runRows)-1)
	return i / pageSize
}

// pageRows is the current page's rows and the cursor's index in them. When
// the page starts mid-section, it gets a copy of that section's header on
// top, so every run on screen sits under its header.
func (l *listPane) pageRows() ([]row, int) {
	if len(l.runRows) == 0 {
		return l.rows, l.cursor
	}
	first := l.page() * pageSize
	last := min(len(l.runRows), first+pageSize) - 1
	start, end := l.runRows[first], l.runRows[last]+1
	if start > 0 && l.rows[start-1].Item == nil {
		return l.rows[start-1 : end], l.cursor - start + 1
	}
	header := ""
	for i := start - 1; i >= 0; i-- {
		if l.rows[i].Item == nil {
			header = l.rows[i].Section
			break
		}
	}
	if header == "" {
		return l.rows[start:end], l.cursor - start
	}
	out := make([]row, 0, end-start+1)
	out = append(out, row{Section: header})
	out = append(out, l.rows[start:end]...)
	return out, l.cursor - start + 1
}

// turnPage moves delta pages and puts the cursor on the first run of the new
// page. It stops at the first and the last page.
func (l *listPane) turnPage(delta int) {
	p := min(max(l.page()+delta, 0), l.pageCount()-1)
	if p == l.page() || len(l.runRows) == 0 {
		return
	}
	l.cursor = l.runRows[p*pageSize]
	l.offset = 0
}

// clampOffset scrolls so the cursor is inside a window of visible rows of the
// current page. When the cursor is the first run of a section, the header
// above it stays in view.
func (l *listPane) clampOffset(visible int) {
	if visible < 1 {
		visible = 1
	}
	rows, cursor := l.pageRows()
	if cursor < l.offset {
		l.offset = cursor
	}
	if cursor >= l.offset+visible {
		l.offset = cursor - visible + 1
	}
	if cursor == l.offset && cursor > 0 && cursor < len(rows) && rows[cursor-1].Item == nil {
		l.offset--
	}
	l.offset = min(l.offset, max(0, len(rows)-visible))
	l.offset = max(l.offset, 0)
}

// emptyMessage explains an empty body.
func (l *listPane) emptyMessage() string {
	if q := l.filter.Value(); q != "" {
		return fmt.Sprintf("no runs match %q · esc clears", q)
	}
	if l.tab != tabAll {
		return "no " + strings.ToLower(l.tab.label()) + " runs"
	}
	return "No sessions yet. Run rival to get started."
}

// view renders the column titles, the visible rows of the current page and
// the page footer as exactly height lines of exactly width cells. It only
// renders the visible window, so 3000 rows cost the same as 30.
func (l *listPane) view(width, height int, spin string) string {
	if width <= 0 || height <= 0 {
		return ""
	}
	c := layoutColumns(width)
	lines := make([]string, 0, height)
	titles := [6]string{"ST", "KIND", "MODEL", "EFF", "TIME", "PROJECT"}
	lines = append(lines, headerStyle.Render(fitCell(joinCells(c, titles, nil), width)))

	// The footer takes the last line whenever there is a line for it.
	bodyEnd := height
	if height >= 2 {
		bodyEnd = height - 1
	}
	rows, cursor := l.pageRows()
	if len(rows) == 0 {
		if len(lines) < bodyEnd {
			lines = append(lines, dimStyle.Render(fitCell(" "+l.emptyMessage(), width)))
		}
	} else {
		for i := l.offset; i < len(rows) && len(lines) < bodyEnd; i++ {
			r := rows[i]
			if r.Item == nil {
				lines = append(lines, sectionStyle.Render(fitCell(" "+r.Section, width)))
				continue
			}
			lines = append(lines, renderRow(r.Item, c, width, spin, i == cursor))
		}
	}
	blank := strings.Repeat(" ", width)
	for len(lines) < bodyEnd {
		lines = append(lines, blank)
	}
	if len(lines) < height {
		lines = append(lines, l.footer(width))
	}
	return strings.Join(lines, "\n")
}

// footerText is the plain page footer: "‹ prev  page P/N  next ›  · T runs",
// or only "T runs" when everything fits on one page.
func (l *listPane) footerText() string {
	runs := runCount(len(l.runRows))
	if n := l.pageCount(); n > 1 {
		return fmt.Sprintf("‹ prev  page %d/%d  next ›  · %s", l.page()+1, n, runs)
	}
	return runs
}

// footer renders footerText as exactly width cells. prev and next are dim when
// there is no page that way; styling is applied only when the text fits, so it
// can never change the width.
func (l *listPane) footer(width int) string {
	plain := " " + l.footerText()
	if l.pageCount() <= 1 || ansi.StringWidth(plain) > width {
		return dimStyle.Render(fitCell(plain, width))
	}
	p, n := l.page(), l.pageCount()
	link := func(s string, ok bool) string {
		if ok {
			return textStyle.Render(s)
		}
		return dimStyle.Render(s)
	}
	out := " " + link("‹ prev", p > 0) + "  " + textStyle.Render(fmt.Sprintf("page %d/%d", p+1, n)) +
		"  " + link("next ›", p < n-1) + dimStyle.Render("  · "+runCount(len(l.runRows)))
	return out + strings.Repeat(" ", width-ansi.StringWidth(plain))
}

// tabBar renders the status tabs with their counts on the left and the filter
// on the right, as exactly width cells.
func (l *listPane) tabBar(width int, loading bool) string {
	if width <= 0 {
		return ""
	}
	var left strings.Builder
	for t := statusTab(0); t < statusTabCount; t++ {
		count := fmt.Sprint(l.counts[t])
		if loading {
			count = "…"
		}
		label := t.label() + " " + count
		style := inactiveTabStyle
		if t == l.tab {
			style = activeTabStyle
		}
		left.WriteString(" ")
		left.WriteString(style.Render(label))
		left.WriteString("  ")
	}
	var right string
	switch {
	case l.filter.Focused():
		right = l.filter.View()
	case l.filter.Value() != "":
		right = dimStyle.Render("/ ") + textStyle.Render(l.filter.Value())
	default:
		right = dimStyle.Render("/ filter")
	}
	lw, rw := lipgloss.Width(left.String()), lipgloss.Width(right)
	switch {
	case lw+rw+1 <= width:
		return joinEnds(left.String(), right+" ", width)
	case l.filter.Focused() || l.filter.Value() != "":
		// Too narrow for both: the filter the user is typing wins.
		return fitCell(" "+right, width)
	default:
		return fitCell(left.String(), width)
	}
}

// runCount is the footer's run total: "1 run", "N runs".
func runCount(n int) string {
	if n == 1 {
		return "1 run"
	}
	return fmt.Sprintf("%d runs", n)
}
