package dashboard

import (
	"strings"
	"testing"
	"time"

	"charm.land/bubbles/v2/spinner"
	tea "charm.land/bubbletea/v2"
	"github.com/1905/rival/internal/session"
	"github.com/charmbracelet/x/ansi"
)

// listFixture spans two sections so the list has header rows to skip.
func listFixture() []*session.Session {
	now := time.Now()
	return []*session.Session{
		{ID: "a0000000-run", CLI: "codex", Model: "gpt-6-astra", Mode: "review", Effort: "xhigh", Status: "running", StartTime: now.Add(-time.Minute), WorkDir: "/src/orbit-web", PromptPreview: "review fingerprint"},
		{ID: "b0000000-done", CLI: "claude", Model: "claude-opus-5-5", Mode: "plan", Effort: "medium", Status: "completed", StartTime: now.Add(-2 * time.Minute), WorkDir: "/src/ledger"},
		{ID: "c0000000-fail", CLI: "codex", Model: "gpt-5.5", Mode: "review", Effort: "high", Status: "failed", StartTime: now.AddDate(0, 0, -40), WorkDir: "/src/orbit-web"},
		{ID: "d0000000-old", CLI: "codex", Model: "gpt-5.5", Mode: "review", Effort: "high", Status: "completed", StartTime: now.AddDate(0, 0, -41), WorkDir: "/src/mathquest"},
	}
}

func newListModel(t *testing.T, sessions []*session.Session, width, height int) Model {
	t.Helper()
	m := New()
	t.Cleanup(m.cancel)
	updated, _ := m.Update(tea.WindowSizeMsg{Width: width, Height: height})
	m = updated.(Model)
	updated, _ = m.Update(SessionEvent{Sessions: sessions})
	return updated.(Model)
}

func send(t *testing.T, m Model, msgs ...tea.Msg) Model {
	t.Helper()
	for _, msg := range msgs {
		updated, _ := m.Update(msg)
		m = updated.(Model)
	}
	return m
}

func typeText(s string) []tea.Msg {
	var out []tea.Msg
	for _, r := range s {
		out = append(out, press(string(r)))
	}
	return out
}

func selectedID(m Model) string {
	if sel := m.list.selected(); sel != nil {
		return sel.Primary().ID
	}
	return ""
}

func TestListMoveSkipsSectionRows(t *testing.T) {
	var l listPane
	l.filter = newFilterInput()
	now := noonToday()
	l.setItems(filterFixture(now), now) // #TODAY a b #YESTERDAY c #OLDER d

	l.top()
	if l.rows[l.cursor].Item == nil || selectedIDOf(&l) != "a" {
		t.Fatalf("top landed on %v", l.rows[l.cursor])
	}
	l.move(1)
	l.move(1) // b -> across #YESTERDAY -> c
	if got := selectedIDOf(&l); got != "c" {
		t.Fatalf("move(1) twice from a = %q, want c", got)
	}
	l.move(-1)
	if got := selectedIDOf(&l); got != "b" {
		t.Fatalf("move(-1) from c = %q, want b", got)
	}
	l.bottom()
	if got := selectedIDOf(&l); got != "d" {
		t.Fatalf("bottom = %q, want d", got)
	}
	l.move(5) // stops at the end
	if got := selectedIDOf(&l); got != "d" {
		t.Fatalf("move past the end = %q, want d", got)
	}
	l.top()
	l.move(-3) // stops at the start, never on the header
	if got := selectedIDOf(&l); got != "a" {
		t.Fatalf("move before the start = %q, want a", got)
	}
}

func selectedIDOf(l *listPane) string {
	if sel := l.selected(); sel != nil {
		return sel.Primary().ID[:1]
	}
	return ""
}

func TestListKeysMoveTheCursor(t *testing.T) {
	m := newListModel(t, listFixture(), 120, 40)
	if got := selectedID(m); got != "a0000000-run" {
		t.Fatalf("initial selection = %q", got)
	}
	m = send(t, m, press("j"), press("j"))
	if got := selectedID(m); got != "c0000000-fail" {
		t.Fatalf("j j = %q, want the failed run across the OLDER header", got)
	}
	m = send(t, m, upperKey('G'))
	if got := selectedID(m); got != "d0000000-old" {
		t.Fatalf("G = %q", got)
	}
	m = send(t, m, press("g"))
	if got := selectedID(m); got != "a0000000-run" {
		t.Fatalf("g = %q", got)
	}
}

// A queued run starting sorts above the selected one; the cursor must follow
// the run, not the index.
func TestListSelectionSurvivesInsertAbove(t *testing.T) {
	sessions := listFixture()
	m := newListModel(t, sessions, 120, 40)
	m = send(t, m, press("j"))
	if got := selectedID(m); got != "b0000000-done" {
		t.Fatalf("setup selection = %q", got)
	}
	fresh := &session.Session{ID: "e0000000-new", CLI: "codex", Model: "gpt-5.5", Status: "running", StartTime: time.Now()}
	m = send(t, m, SessionEvent{Sessions: append([]*session.Session{fresh}, sessions...)})
	if got := selectedID(m); got != "b0000000-done" {
		t.Fatalf("selection after insert above = %q, want b0000000-done", got)
	}
}

func TestFilterNarrowsClearsAndKeeps(t *testing.T) {
	m := newListModel(t, listFixture(), 120, 40)

	m = send(t, m, press("/"))
	if m.mode != modeFilter || !m.list.filter.Focused() {
		t.Fatal("/ did not focus the filter")
	}
	m = send(t, m, typeText("orbit")...)
	if got := len(itemRows(m.list.rows)); got != 2 {
		t.Fatalf("filter orbit left %d runs, want 2", got)
	}

	// esc clears and blurs.
	m = send(t, m, press("esc"))
	if m.mode != modeList || m.list.filter.Focused() || m.list.filter.Value() != "" {
		t.Fatalf("esc: mode=%d focused=%v value=%q", m.mode, m.list.filter.Focused(), m.list.filter.Value())
	}
	if got := len(itemRows(m.list.rows)); got != 4 {
		t.Fatalf("after esc %d runs, want 4", got)
	}

	// enter keeps the filter and blurs.
	m = send(t, m, press("/"))
	m = send(t, m, typeText("mathquest")...)
	m = send(t, m, press("enter"))
	if m.mode != modeList || m.list.filter.Focused() || m.list.filter.Value() != "mathquest" {
		t.Fatalf("enter: mode=%d focused=%v value=%q", m.mode, m.list.filter.Focused(), m.list.filter.Value())
	}
	if got := selectedID(m); got != "d0000000-old" {
		t.Fatalf("filtered selection = %q, want the only match", got)
	}
	// Now j/k navigate again, and esc in the list clears the kept filter.
	m = send(t, m, press("esc"))
	if m.list.filter.Value() != "" || len(itemRows(m.list.rows)) != 4 {
		t.Fatal("esc in the list did not clear the kept filter")
	}
}

func itemRows(rows []row) []row {
	var out []row
	for _, r := range rows {
		if r.Item != nil {
			out = append(out, r)
		}
	}
	return out
}

func TestQInsideFilterTypesQ(t *testing.T) {
	m := newListModel(t, listFixture(), 120, 40)
	updated, _ := m.Update(press("/"))
	m = updated.(Model)
	updated, cmd := m.Update(press("q"))
	m = updated.(Model)
	if m.quitting {
		t.Fatal("q inside the filter quit the TUI")
	}
	if cmd != nil {
		if _, ok := cmd().(tea.QuitMsg); ok {
			t.Fatal("q inside the filter returned tea.Quit")
		}
	}
	if m.list.filter.Value() != "q" {
		t.Fatalf("filter value = %q, want q", m.list.filter.Value())
	}
	// ctrl+c still quits from inside the input.
	updated, _ = m.Update(press("ctrl+c"))
	if !updated.(Model).quitting {
		t.Fatal("ctrl+c inside the filter did not quit")
	}
}

func TestStatusTabsCycleAndShowCounts(t *testing.T) {
	m := newListModel(t, listFixture(), 120, 40)
	bar := ansi.Strip(m.list.tabBar(120, false))
	for _, want := range []string{"ALL 4", "RUNNING 1", "FAILED 1", "DONE 2"} {
		if !strings.Contains(bar, want) {
			t.Fatalf("tab bar %q omits %q", bar, want)
		}
	}

	m = send(t, m, press("tab"))
	if m.list.tab != tabRunning || len(itemRows(m.list.rows)) != 1 {
		t.Fatalf("tab -> %v with %d runs", m.list.tab, len(itemRows(m.list.rows)))
	}
	m = send(t, m, press("tab"))
	if m.list.tab != tabFailed || selectedID(m) != "c0000000-fail" {
		t.Fatalf("tab tab -> %v selected %q", m.list.tab, selectedID(m))
	}
	m = send(t, m, press("shift+tab"), press("shift+tab"), press("shift+tab"))
	if m.list.tab != tabDone {
		t.Fatalf("shift+tab x3 from FAILED -> %v, want DONE (wraps)", m.list.tab)
	}

	// Counts follow the filter.
	m = send(t, m, press("/"))
	m = send(t, m, typeText("orbit")...)
	bar = ansi.Strip(m.list.tabBar(120, false))
	for _, want := range []string{"ALL 2", "RUNNING 1", "FAILED 1", "DONE 0"} {
		if !strings.Contains(bar, want) {
			t.Fatalf("filtered tab bar %q omits %q", bar, want)
		}
	}
}

func TestEmptyFilterResultExplainsItself(t *testing.T) {
	m := newListModel(t, listFixture(), 120, 40)
	m = send(t, m, press("/"))
	m = send(t, m, typeText("zzz")...)
	if !strings.Contains(ansi.Strip(m.viewContent()), `no runs match "zzz" · esc clears`) {
		t.Fatalf("empty result message missing:\n%s", ansi.Strip(m.viewContent()))
	}
	if m.list.selected() != nil {
		t.Fatal("an empty list still reports a selection")
	}
	// enter on an empty list must not open a detail view.
	m = send(t, m, press("enter"), press("enter"))
	if m.mode != modeList {
		t.Fatalf("enter on an empty list switched to mode %d", m.mode)
	}
}

func TestHelpToggle(t *testing.T) {
	m := newListModel(t, listFixture(), 120, 40)
	short := m.helpView()
	m = send(t, m, press("?"))
	if !m.help.ShowAll {
		t.Fatal("? did not expand help")
	}
	if full := m.helpView(); strings.Count(full, "\n") == 0 || full == short {
		t.Fatalf("full help did not expand:\n%s", full)
	}
	m = send(t, m, press("?"))
	if m.help.ShowAll {
		t.Fatal("? did not collapse help")
	}
}

func TestSpinnerTicksOnlyWhileRunning(t *testing.T) {
	m := newListModel(t, listFixture(), 120, 40)
	if !m.spinning {
		t.Fatal("a running session did not start the spinner")
	}
	before := m.spin.View()
	updated, cmd := m.Update(spinner.TickMsg{ID: m.spin.ID()})
	m = updated.(Model)
	if cmd == nil || m.spin.View() == before {
		t.Fatal("spinner did not advance while a run is running")
	}

	// Everything finishes: the next tick stops the chain.
	done := listFixture()
	for _, s := range done {
		if s.Status == "running" {
			s.Status = "completed"
		}
	}
	m = send(t, m, SessionEvent{Sessions: done})
	updated, cmd = m.Update(spinner.TickMsg{ID: m.spin.ID()})
	m = updated.(Model)
	if cmd != nil || m.spinning {
		t.Fatal("spinner kept ticking with nothing running")
	}

	// A new running run restarts it.
	m = send(t, m, SessionEvent{Sessions: listFixture()})
	if !m.spinning {
		t.Fatal("spinner did not restart for a new running run")
	}
}

func TestTinyTerminalShowsNotice(t *testing.T) {
	m := newListModel(t, listFixture(), 50, 20)
	if got := m.viewContent(); got != "terminal too small (need 60×16)" {
		t.Fatalf("tiny frame = %q", got)
	}
}

func TestCompactHeaderBelowThirtyRows(t *testing.T) {
	m := newListModel(t, listFixture(), 120, 29)
	frame := m.viewContent()
	lines := strings.Split(frame, "\n")
	if len(lines) != 29 {
		t.Fatalf("frame has %d lines, want 29", len(lines))
	}
	if !strings.Contains(ansi.Strip(lines[1]), "ALL 4") {
		t.Fatalf("line 1 is not the tab bar, so the header did not collapse: %q", ansi.Strip(lines[1]))
	}
}

// A bracketed paste reaches the filter as a PasteMsg, not a key; it must
// filter the rows like typing does.
func TestPasteIntoFilterRefilters(t *testing.T) {
	m := newListModel(t, previewFixture(t), 120, 40)
	m = send(t, m, press("/"), tea.PasteMsg{Content: "mathquest"})
	for _, r := range m.list.rows {
		if r.Item != nil && projectName(r.Item.Primary().WorkDir) != "mathquest" {
			t.Fatalf("paste left %s in the list", projectName(r.Item.Primary().WorkDir))
		}
	}
}

// Only one 1s refresh chain may exist: repeated events must not start more.
func TestSingleRefreshTickChain(t *testing.T) {
	m := newListModel(t, previewFixture(t), 120, 40)
	if !m.ticking {
		t.Fatal("a running row did not start the refresh tick")
	}
	m = send(t, m, SessionEvent{Sessions: previewFixture(t)})
	if !m.ticking {
		t.Fatal("ticking flag lost")
	}
	done := previewFixture(t)[1:]
	m = send(t, m, SessionEvent{Sessions: done}, tickMsg(time.Now()))
	if m.ticking {
		t.Fatal("tick chain did not end once nothing runs")
	}
}
