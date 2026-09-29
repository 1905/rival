package dashboard

import (
	"fmt"
	"strings"
	"time"
	"unicode"

	"charm.land/bubbles/v2/key"
	"charm.land/bubbles/v2/textinput"
	"charm.land/bubbles/v2/viewport"
	"charm.land/lipgloss/v2"
	"github.com/1905/rival/internal/session"
	"github.com/charmbracelet/x/ansi"
)

// detailTab is the tab shown on the detail screen.
type detailTab int

const (
	tabOutput detailTab = iota
	tabPrompt
	tabInfo
)

// detailTabCount is the number of detailTab values, for cycling.
const detailTabCount = 3

func (t detailTab) label() string {
	switch t {
	case tabPrompt:
		return "Prompt"
	case tabInfo:
		return "Info"
	default:
		return "Output"
	}
}

// detailChromeH is the rows the detail screen draws around its viewport:
// breadcrumb, tab bar, separator, separator, status line. The help bar is
// counted by the caller, which owns its height.
const detailChromeH = 5

// killConfirm is an open "stop N running sessions? y/n" bar. targets are the
// sessions that were live when x was pressed; y re-checks them against the
// current snapshot before it sends anything.
type killConfirm struct{ targets []*session.Session }

// detailPane is the full-screen view of one run: tabs, group members, a
// follow-able viewport, search and the kill confirm.
type detailPane struct {
	tab detailTab
	// memberID is the member the tabs show. It is an ID, not an index: a
	// group that gains or loses a session reorders its slice, and an index
	// would silently switch to another member.
	memberID string
	vp       viewport.Model
	follow   bool

	search   textinput.Model
	query    string
	matches  []int
	matchIdx int

	confirm *killConfirm
	prompts map[string]string

	// lines is the current tab's content before search highlighting, so a
	// new query never has to re-read the log. log caches the Output tab's
	// read by file state.
	lines []string
	log   logCache
	// notice is a one-shot status message ("nothing running"). The next key
	// clears it.
	notice string
}

func newDetailPane() detailPane {
	vp := viewport.New(viewport.WithWidth(0), viewport.WithHeight(0))
	// "f" is follow here, not page down.
	vp.KeyMap.PageDown = key.NewBinding(key.WithKeys("pgdown", "space"))
	return detailPane{vp: vp, search: newSearchInput()}
}

// newSearchInput builds the "/" search input in the phosphor palette.
func newSearchInput() textinput.Model {
	ti := newFilterInput()
	ti.Placeholder = "search"
	return ti
}

// open resets the pane for item: Output tab, first member, follow on, no
// search. It loads the full prompts, which the list's summaries drop.
func (d *detailPane) open(item *displayItem) {
	d.tab = tabOutput
	d.memberID = ""
	if item != nil && len(item.Sessions) > 0 {
		d.memberID = item.Sessions[0].ID
	}
	d.follow = true
	d.clearSearch()
	d.confirm = nil
	d.notice = ""
	d.prompts = loadPrompts(item)
}

// close drops everything that belongs to one run.
func (d *detailPane) close() {
	d.prompts = nil
	d.lines = nil
	d.log = logCache{}
	d.confirm = nil
	d.notice = ""
	d.clearSearch()
	d.vp.SetContent("")
}

func (d *detailPane) clearSearch() {
	d.query = ""
	d.matches = nil
	d.matchIdx = 0
	d.search.Reset()
	d.search.Blur()
}

// loadPrompts reads the full prompt of every member of item. A session that
// cannot be read is left out, and the Prompt tab falls back to its preview.
func loadPrompts(item *displayItem) map[string]string {
	if item == nil {
		return nil
	}
	prompts := make(map[string]string, len(item.Sessions))
	for _, s := range item.Sessions {
		if s.Prompt != "" {
			prompts[s.ID] = s.Prompt
			continue
		}
		full, err := session.Load(s.ID)
		if err != nil || full == nil || full.Prompt == "" {
			continue
		}
		prompts[s.ID] = full.Prompt
	}
	return prompts
}

// memberIndex is the position of memberID in item, or 0 when it is gone.
func (d *detailPane) memberIndex(item *displayItem) int {
	for i, s := range item.Sessions {
		if s.ID == d.memberID {
			return i
		}
	}
	return 0
}

// current is the member the tabs show, or nil for an empty item.
func (d *detailPane) current(item *displayItem) *session.Session {
	if item == nil || len(item.Sessions) == 0 {
		return nil
	}
	return item.Sessions[d.memberIndex(item)]
}

// cycleMember moves to the next (+1) or previous (-1) member, wrapping.
func (d *detailPane) cycleMember(item *displayItem, delta int) {
	if item == nil || len(item.Sessions) < 2 {
		return
	}
	n := len(item.Sessions)
	d.memberID = item.Sessions[((d.memberIndex(item)+delta)%n+n)%n].ID
}

// vpHeight is the viewport height for a pane of height rows. resize and view
// both call it, so the viewport never renders more rows than the view keeps.
func vpHeight(height int) int {
	return max(1, height-detailChromeH)
}

// resize fits the viewport to a width×height pane. A reader following the
// tail stays on it; anyone else keeps their place, clamped to the new end.
func (d *detailPane) resize(width, height int) {
	d.vp.SetWidth(width)
	d.vp.SetHeight(vpHeight(height))
	if d.tab == tabOutput && d.follow {
		d.vp.GotoBottom()
		return
	}
	d.vp.SetYOffset(d.vp.YOffset())
}

// reload rebuilds the current tab's content for item at width. resetBottom
// resets the scroll position: Output jumps to the tail, the other tabs to the
// top. Otherwise Output follows the tail only while follow is on, and a
// scrolled-up reader keeps their place.
func (d *detailPane) reload(item *displayItem, width int, resetBottom bool) {
	s := d.current(item)
	if s == nil || width <= 0 {
		d.lines = nil
		d.vp.SetContent("")
		return
	}
	// A vanished member fell back to the first; stay there if it returns.
	d.memberID = s.ID

	switch d.tab {
	case tabPrompt:
		d.lines = promptLines(s, d.prompts, width)
	case tabInfo:
		d.lines = infoLines(s, width)
	default:
		d.lines = outputLines(s, width, &d.log)
	}
	d.applyContent()

	switch {
	case d.tab == tabOutput && (resetBottom || d.follow):
		// Landing on the tail by a reset means following it too.
		d.follow = true
		d.vp.GotoBottom()
	case resetBottom:
		d.vp.GotoTop()
	}
}

// applyContent pushes lines into the viewport, highlighting search matches.
// SetContent clamps the offset, so a shrinking log never leaves the viewport
// scrolled past its end.
func (d *detailPane) applyContent() {
	d.matches = findMatches(d.lines, d.query)
	if d.matchIdx >= len(d.matches) {
		d.matchIdx = 0
	}
	out := make([]string, len(d.lines))
	copy(out, d.lines)
	for _, i := range d.matches {
		out[i] = highlightLine(out[i], d.query)
	}
	d.vp.SetContentLines(out)
}

// runSearch sets the query and jumps to its first match.
func (d *detailPane) runSearch(query string) {
	d.query = strings.TrimSpace(query)
	d.matchIdx = 0
	d.applyContent()
	d.jumpToMatch()
}

// stepMatch moves to the next (+1) or previous (-1) match, wrapping.
func (d *detailPane) stepMatch(delta int) {
	n := len(d.matches)
	if n == 0 {
		return
	}
	d.matchIdx = ((d.matchIdx+delta)%n + n) % n
	d.jumpToMatch()
}

// jumpToMatch scrolls the current match into view, a third of the way down.
// Reading a match means reading old output, so follow pauses.
func (d *detailPane) jumpToMatch() {
	if len(d.matches) == 0 {
		return
	}
	line := d.matches[d.matchIdx]
	d.vp.SetYOffset(line - d.vp.Height()/3)
	// Not AtBottom(): a match near the tail clamps to the bottom, and turning
	// follow on there would scroll the match away on the next output.
	d.follow = false
}

// outputLines is the member's log, pre-wrapped to width, followed by its error
// when it failed. The error goes last because follow parks the reader there.
func outputLines(s *session.Session, width int, cache *logCache) []string {
	lines, err := cache.read(s, width, 0)
	switch {
	case err != nil:
		lines = []string{dimStyle.Render(fmt.Sprintf("(log unavailable: %v)", err))}
	case len(lines) == 0:
		lines = []string{dimStyle.Render("(empty log)")}
	}
	if s.Status == "failed" {
		lines = append(lines, errorLines(s, width)...)
	}
	return lines
}

// errorLines is s's error under an "error:" heading, wrapped to width, or nil
// when it has none.
func errorLines(s *session.Session, width int) []string {
	if s.ErrorMsg == "" {
		return nil
	}
	out := []string{"", failedStyle.Render("error:")}
	for _, l := range wrapCells(sanitizeLog(s.ErrorMsg), width) {
		out = append(out, failedStyle.Render(l))
	}
	return out
}

// promptLines is the member's full prompt, word-wrapped to width. When the
// stored record cannot be read it is the 100-char preview plus a note.
func promptLines(s *session.Session, prompts map[string]string, width int) []string {
	if full := prompts[s.ID]; full != "" {
		return wrapCells(sanitizeLog(full), width)
	}
	lines := wrapCells(sanitizeLog(s.PromptPreview), width)
	if s.PromptPreview == "" {
		lines = nil
	}
	return append(lines, dimStyle.Render("(full prompt unavailable)"))
}

// infoLabelW is the label column of the Info tab.
const infoLabelW = 11

// infoLines lists every stored field of s, one per row, then the full error.
// Values wrap under their label rather than being cut.
func infoLines(s *session.Session, width int) []string {
	valW := max(1, width-infoLabelW-1)
	indent := strings.Repeat(" ", infoLabelW+1)
	var out []string
	add := func(label, value string, style lipgloss.Style) {
		if value == "" {
			value = "-"
		}
		for i, l := range wrapCells(value, valW) {
			prefix := indent
			if i == 0 {
				prefix = dimStyle.Render(fitCell(label, infoLabelW)) + " "
			}
			out = append(out, prefix+style.Render(l))
		}
	}
	ts := func(t *time.Time) string {
		if t == nil || t.IsZero() {
			return ""
		}
		return t.Local().Format("2006-01-02 15:04:05")
	}
	exit := ""
	if s.ExitCode != nil {
		exit = fmt.Sprintf("%d", *s.ExitCode)
	}
	pid := ""
	if s.PID > 0 {
		pid = fmt.Sprintf("%d", s.PID)
	}
	start := s.StartTime

	add("id", s.ID, valueStyle)
	add("group id", s.GroupID, textStyle)
	add("cli", s.CLI, textStyle)
	add("model", modelName(s), valueStyle)
	add("effort", s.Effort, textStyle)
	add("mode", s.Mode, textStyle)
	add("status", s.Status, statusStyle(s.Status))
	add("exit", exit, textStyle)
	add("started", ts(&start), textStyle)
	add("ended", ts(s.EndTime), textStyle)
	add("duration", formatElapsed(s), textStyle)
	add("queued at", ts(s.QueuedAt), textStyle)
	add("workdir", s.WorkDir, textStyle)
	add("scope", s.ReviewScope, textStyle)
	add("account", s.Account, textStyle)
	add("pid", pid, textStyle)
	add("output", fmt.Sprintf("%d bytes, %d lines", s.OutputBytes, s.OutputLines), textStyle)
	add("log", s.LogFile, textStyle)
	return append(out, errorLines(s, width)...)
}

// wrapCells word-wraps text to width display cells and hard-breaks anything
// still longer, so no line ever exceeds width.
func wrapCells(text string, width int) []string {
	if width <= 0 {
		return []string{text}
	}
	return strings.Split(ansi.Hardwrap(ansi.Wordwrap(text, width, ""), width, true), "\n")
}

// findMatches returns the indexes of the lines that contain query, ignoring
// case and styling. An empty query matches nothing.
func findMatches(lines []string, query string) []int {
	q := strings.ToLower(query)
	if q == "" {
		return nil
	}
	var out []int
	for i, l := range lines {
		if strings.Contains(strings.ToLower(ansi.Strip(l)), q) {
			out = append(out, i)
		}
	}
	return out
}

// matchStyle marks a search hit. Colours only, so the line keeps its width.
var matchStyle = lipgloss.NewStyle().Foreground(lipgloss.Color("#000000")).Background(colAccent)

// highlightLine gives every case-insensitive occurrence of query in line the
// accent background. Ranges are in display cells, which StyleRanges expects.
func highlightLine(line, query string) string {
	q := []rune(strings.ToLower(query))
	if len(q) == 0 {
		return line
	}
	plain := []rune(ansi.Strip(line))
	cells := make([]int, len(plain)+1) // cells[i] = cell offset of rune i
	for i, r := range plain {
		cells[i+1] = cells[i] + ansi.StringWidth(string(r))
	}
	var ranges []lipgloss.Range
	for i := 0; i+len(q) <= len(plain); {
		hit := true
		for j, qr := range q {
			if unicode.ToLower(plain[i+j]) != qr {
				hit = false
				break
			}
		}
		if !hit {
			i++
			continue
		}
		ranges = append(ranges, lipgloss.NewRange(cells[i], cells[i+len(q)], matchStyle))
		i += len(q)
	}
	return lipgloss.StyleRanges(line, ranges...)
}

// view renders the whole detail screen for item: exactly height lines, none
// wider than width. The viewport already holds the content; view reads no
// file.
func (d *detailPane) view(item *displayItem, width, height int, spin string) string {
	if item == nil || item.Primary() == nil || width <= 0 || height <= 0 {
		return ""
	}
	sep := dimStyle.Render(strings.Repeat("─", width))
	lines := []string{
		d.breadcrumb(item, width, spin),
		d.tabBar(item, width),
		sep,
	}
	lines = append(lines, padLines(d.vp.View(), width, vpHeight(height))...)
	lines = append(lines, sep, d.statusLine(width))
	if len(lines) > height {
		lines = lines[:height]
	}
	return strings.Join(lines, "\n")
}

// padLines splits s and returns exactly n lines of at most width cells.
func padLines(s string, width, n int) []string {
	src := strings.Split(s, "\n")
	out := make([]string, n)
	for i := range out {
		if i < len(src) {
			out[i] = ansi.Truncate(src[i], width, "")
		}
	}
	return out
}

// breadcrumb reads " rival › project › kind id8" on the left and
// "glyph status elapsed   follow ●" on the right.
func (d *detailPane) breadcrumb(item *displayItem, width int, spin string) string {
	s := item.Primary()
	id := s.ID
	if s.GroupID != "" {
		id = s.GroupID
	}
	arrow := dimStyle.Render(" › ")
	left := " " + rivalWord() + arrow + textStyle.Render(projectName(s.WorkDir)) + arrow +
		textStyle.Render(kindLabel(item)+" "+shortID(id))

	status := itemStatus(item)
	elapsed := formatElapsed(s)
	if item.IsGroup() {
		elapsed = groupElapsed(item)
	}
	glyph := statusGlyph(status, spin)
	if glyph == "" {
		glyph = "●"
	}
	followMark := "○"
	if d.follow {
		followMark = "●"
	}
	right := statusStyle(status).Render(glyph+" "+status+" "+elapsed) + "   " +
		dimStyle.Render("follow ") + textStyle.Render(followMark) + " "
	return joinEnds(left, right, width)
}

// joinEnds puts left and right at the two ends of a width-cell line. When both
// do not fit, left is cut first: the status on the right matters more.
func joinEnds(left, right string, width int) string {
	lw, rw := lipgloss.Width(left), lipgloss.Width(right)
	if rw >= width {
		return ansi.Truncate(right, width, "")
	}
	if lw+rw > width {
		left = ansi.Truncate(left, width-rw, "…")
		lw = lipgloss.Width(left)
	}
	return left + strings.Repeat(" ", width-lw-rw) + right
}

// tabBar shows the tabs on the left and, for a group, the member bar on the
// right: " [ Output ]  Prompt  Info        [ gpt-5.5 ] gemini-3.1  judge".
func (d *detailPane) tabBar(item *displayItem, width int) string {
	var left strings.Builder
	for t := detailTab(0); t < detailTabCount; t++ {
		left.WriteString(" ")
		if t == d.tab {
			left.WriteString(activeTabStyle.Render("[ " + t.label() + " ]"))
		} else {
			left.WriteString(inactiveTabStyle.Render(t.label()))
		}
		left.WriteString(" ")
	}
	if !item.IsGroup() {
		return ansi.Truncate(left.String(), width, "")
	}
	var right strings.Builder
	cur := d.memberIndex(item)
	for i, s := range item.Sessions {
		label := memberLabel(s)
		if i == cur {
			right.WriteString(activeTabStyle.Render("[ " + label + " ]"))
		} else {
			right.WriteString(inactiveTabStyle.Render(label))
		}
		right.WriteString(" ")
	}
	l, r := left.String(), right.String()
	if lipgloss.Width(l)+lipgloss.Width(r)+2 > width {
		// Too narrow for both at the ends: members follow the tabs directly
		// and the tail is cut.
		return ansi.Truncate(l+" "+r, width, "…")
	}
	return joinEnds(l, r, width)
}

// memberLabel is a member's tab name: its model id, or "judge" for the
// consilium judge.
func memberLabel(s *session.Session) string {
	if s.Mode == "consilium" {
		return "judge"
	}
	return modelName(s)
}

// statusLine is the row under the viewport: the confirm bar, the search input,
// the search result, a one-shot notice, or the scroll position.
func (d *detailPane) statusLine(width int) string {
	var line string
	switch {
	case d.confirm != nil:
		n := len(d.confirm.targets)
		noun := "sessions"
		if n == 1 {
			noun = "session"
		}
		line = " " + runningStyle.Render(fmt.Sprintf("stop %d running %s? y/n", n, noun))
	case d.search.Focused():
		line = " " + d.search.View()
	case d.query != "":
		result := dimStyle.Render("no matches · esc clear")
		if n := len(d.matches); n > 0 {
			result = textStyle.Render(fmt.Sprintf("%d/%d matches", d.matchIdx+1, n)) +
				dimStyle.Render(" · n next · N prev · esc clear")
		}
		line = " " + textStyle.Render("/"+d.query) + "  " + result
	case d.notice != "":
		line = " " + dimStyle.Render(d.notice)
	default:
		total := d.vp.TotalLineCount()
		if total > 0 {
			first := d.vp.YOffset() + 1
			last := min(total, d.vp.YOffset()+d.vp.Height())
			line = dimStyle.Render(fmt.Sprintf(" lines %d-%d of %d", first, last, total))
		}
	}
	return ansi.Truncate(line, width, "")
}

// liveTargets returns the members of item a stop would signal: running or
// queued, with a known PID whose process alive confirms is still this run's.
// Queued sessions carry the waiting rival process's PID, so SIGTERM cancels
// the queue wait. A stale "running" record whose process is gone is left out.
func liveTargets(item *displayItem, alive func(pid int, start int64) bool) []*session.Session {
	if item == nil {
		return nil
	}
	var out []*session.Session
	for _, s := range item.Sessions {
		if isLive(s.Status) && mayStop(s, alive) {
			out = append(out, s)
		}
	}
	return out
}

// unverifiedNotice explains why a live-looking run got no signal.
const unverifiedNotice = "cannot verify process identity"

// mayStop reports whether s may get SIGTERM: a recorded non-zero PIDStart that
// alive confirms still matches the process. Without a start time the PID could
// belong to any process that reused it, so it is never signalled.
func mayStop(s *session.Session, alive func(pid int, start int64) bool) bool {
	return s.PID > 0 && s.PIDStart != 0 && alive(s.PID, s.PIDStart)
}

// hasUnverified reports whether item has a live member with a PID but no
// recorded start time: one a stop refuses to signal.
func hasUnverified(item *displayItem) bool {
	if item == nil {
		return false
	}
	for _, s := range item.Sessions {
		if isLive(s.Status) && s.PID > 0 && s.PIDStart == 0 {
			return true
		}
	}
	return false
}
