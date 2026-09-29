package dashboard

import (
	"fmt"
	"os"
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/1905/rival/internal/config"
	"github.com/1905/rival/internal/session"
	"github.com/charmbracelet/x/ansi"
)

// press builds a KeyPressMsg for a single-rune or named key.
func press(s string) tea.KeyPressMsg {
	if len([]rune(s)) == 1 {
		return tea.KeyPressMsg{Code: []rune(s)[0], Text: s}
	}
	switch s {
	case "enter":
		return tea.KeyPressMsg{Code: tea.KeyEnter}
	case "esc":
		return tea.KeyPressMsg{Code: tea.KeyEscape}
	case "tab":
		return tea.KeyPressMsg{Code: tea.KeyTab}
	case "shift+tab":
		return tea.KeyPressMsg{Code: tea.KeyTab, Mod: tea.ModShift}
	case "ctrl+c":
		return tea.KeyPressMsg{Code: 'c', Mod: tea.ModCtrl}
	case "pgdown":
		return tea.KeyPressMsg{Code: tea.KeyPgDown}
	case "pgup":
		return tea.KeyPressMsg{Code: tea.KeyPgUp}
	default:
		panic("unhandled key " + s)
	}
}

// upperKey builds a shifted single-rune keypress (e.g. "G").
func upperKey(r rune) tea.KeyPressMsg {
	return tea.KeyPressMsg{Code: r, Text: string(r), Mod: tea.ModShift}
}

func writeTempLog(t *testing.T, name, content string) string {
	t.Helper()
	path := t.TempDir() + "/" + name
	if err := os.WriteFile(path, []byte(content), 0o600); err != nil {
		t.Fatal(err)
	}
	return path
}

// nastyLog mixes tabs, CJK, and ANSI escapes — everything that made the old
// rune-counting wrap overflow the terminal.
const nastyLog = "\tfunc main() {\n\t\tfmt.Println(\"日本語のテキストはとても幅が広いですね、これは長い行です\")\n\x1b[31m\terror: something went terribly wrong in a very long line that must wrap\x1b[0m\n\t}\n"

func newTestModel(t *testing.T, logBody string) Model {
	t.Helper()
	m := New()
	t.Cleanup(m.cancel)
	m.loaded = true
	m.list.setItems([]displayItem{{Sessions: []*session.Session{{
		ID:        "widthtest",
		CLI:       "codex",
		Model:     config.GPT56SolModel,
		Mode:      "review",
		Effort:    "ultra",
		Status:    "running",
		StartTime: time.Now(),
		Prompt:    strings.Repeat("review this repository carefully ", 40),
		LogFile:   writeTempLog(t, "widthtest.log", logBody),
	}}}}, time.Now())
	return m
}

func assertFrameWidth(t *testing.T, m Model, label string) {
	t.Helper()
	for i, line := range strings.Split(m.viewContent(), "\n") {
		if w := ansi.StringWidth(line); w > m.lay.Width {
			t.Fatalf("%s: line %d width %d exceeds terminal width %d: %q", label, i, w, m.lay.Width, line)
		}
	}
}

func TestViewNeverExceedsWidth(t *testing.T) {
	body := nastyLog + strings.Repeat("padding line to make the log long enough to scroll\n", 200)

	for _, width := range []int{60, 90, 120, 200} {
		m := newTestModel(t, body)

		updated, _ := m.Update(tea.WindowSizeMsg{Width: width, Height: 24})
		m = updated.(Model)
		assertFrameWidth(t, m, "list mode")
		if n := strings.Count(m.viewContent(), "\n") + 1; n != 24 {
			t.Fatalf("width %d: list frame has %d lines, want 24", width, n)
		}

		// Expanded help borrows rows from the list, never from the frame.
		updated, _ = m.Update(press("?"))
		m = updated.(Model)
		assertFrameWidth(t, m, "list mode, full help")
		if n := strings.Count(m.viewContent(), "\n") + 1; n != 24 {
			t.Fatalf("width %d: list frame with full help has %d lines, want 24", width, n)
		}

		updated, _ = m.Update(press("enter"))
		m = updated.(Model)
		if m.mode != modeDetail {
			t.Fatalf("width %d: enter did not switch to detail mode", width)
		}
		assertFrameWidth(t, m, "detail mode")
		for _, k := range []string{"2", "3", "1", "/"} {
			m = send(t, m, press(k))
			assertFrameWidth(t, m, "detail tab "+k)
			if n := strings.Count(m.viewContent(), "\n") + 1; n != 24 {
				t.Fatalf("width %d: detail frame after %q has %d lines, want 24", width, k, n)
			}
		}
	}
}

// The detail frame fills the terminal exactly, at every width and with the
// help bar short or expanded, for a solo run and a group.
func TestDetailFrameFitsTheTerminal(t *testing.T) {
	for _, width := range []int{200, 120, 90, 60} {
		for _, height := range []int{50, 29, 16} {
			for _, fixture := range [][]*session.Session{previewFixture(t), groupFixture(t)} {
				m := openDetail(t, fixture, width, height)
				for _, step := range []string{"", "?", "2", "3", "x"} {
					if step != "" {
						m = send(t, m, press(step))
					}
					label := fmt.Sprintf("%dx%d after %q", width, height, step)
					assertFrameWidth(t, m, label)
					if n := strings.Count(m.viewContent(), "\n") + 1; n != height {
						t.Fatalf("%s: frame has %d lines, want %d", label, n, height)
					}
				}
			}
		}
	}
}

func TestEnterEscKeepsTheCursor(t *testing.T) {
	m := newListModel(t, previewFixture(t), 120, 40)
	m = send(t, m, press("j"))
	want := selectedID(m)
	m = send(t, m, press("enter"))
	if m.mode != modeDetail {
		t.Fatal("enter did not open detail")
	}
	m = send(t, m, press("esc"))
	if m.mode != modeList || selectedID(m) != want {
		t.Fatalf("esc: mode %d, selected %q, want list on %q", m.mode, selectedID(m), want)
	}
}

func TestDetailScrollKeysReachTheViewport(t *testing.T) {
	m := newTestModel(t, strings.Repeat("scrollable line\n", 300))

	updated, _ := m.Update(tea.WindowSizeMsg{Width: 80, Height: 24})
	m = updated.(Model)
	updated, _ = m.Update(press("enter"))
	m = updated.(Model)

	before := m.detail.vp.YOffset()
	cursorBefore := m.list.cursor
	updated, _ = m.Update(press("k"))
	m = updated.(Model)
	if m.detail.vp.YOffset() >= before {
		t.Fatalf("k did not scroll the log up (offset %d -> %d)", before, m.detail.vp.YOffset())
	}
	if m.list.cursor != cursorBefore {
		t.Fatalf("k moved the list cursor in detail mode: %d -> %d", cursorBefore, m.list.cursor)
	}

	// g jumps to the top of the log, not the top of the list.
	updated, _ = m.Update(press("g"))
	m = updated.(Model)
	if !m.detail.vp.AtTop() {
		t.Fatal("g did not jump the viewport to the top")
	}
}

// A queued run starting while a detail view is open re-sorts the list (LoadAll
// orders by StartTime, MarkRunning stamps it at launch). The selection must
// follow the session the user opened, not the index it happened to occupy.
func TestDetailSelectionSurvivesReorder(t *testing.T) {
	watched := &session.Session{
		ID: "11111111-1111-1111-1111-111111111111", CLI: "codex", Model: config.GPT56SolModel,
		Mode: "review", Status: "running", StartTime: time.Now().Add(-time.Minute), PID: 4242,
		LogFile: writeTempLog(t, "watched.log", strings.Repeat("watched output\n", 50)),
	}
	other := &session.Session{
		ID: "22222222-2222-2222-2222-222222222222", CLI: "claude", Model: config.ClaudeModel,
		Mode: "review", Status: "queued", StartTime: time.Now().Add(-2 * time.Minute), PID: 9999,
		LogFile: writeTempLog(t, "other.log", strings.Repeat("other output\n", 50)),
	}

	m := New()
	t.Cleanup(m.cancel)
	updated, _ := m.Update(SessionEvent{Sessions: []*session.Session{watched, other}})
	m = updated.(Model)
	updated, _ = m.Update(tea.WindowSizeMsg{Width: 80, Height: 24})
	m = updated.(Model)
	updated, _ = m.Update(press("enter"))
	m = updated.(Model)

	if got := m.list.selected().Primary().ID; got != watched.ID {
		t.Fatalf("opened the wrong session: %s", got)
	}

	// The queued run starts and sorts above the watched one.
	other.Status = "running"
	other.StartTime = time.Now()
	updated, _ = m.Update(SessionEvent{Sessions: []*session.Session{other, watched}})
	m = updated.(Model)

	if m.mode != modeDetail {
		t.Fatal("reorder dropped the user out of the detail view")
	}
	if got := m.list.selected().Primary().ID; got != watched.ID {
		t.Fatalf("detail view followed the index, not the session: showing %s, want %s", got, watched.ID)
	}
	if got := m.list.selected().Primary().PID; got != watched.PID {
		t.Fatalf("x would target pid %d instead of %d", got, watched.PID)
	}
}

// A session that disappears entirely returns the user to the list rather than
// leaving another run's log under the old heading.
func TestDetailExitsWhenSelectionDisappears(t *testing.T) {
	gone := &session.Session{
		ID: "33333333-3333-3333-3333-333333333333", CLI: "codex", Model: config.GPT56SolModel,
		Mode: "review", Status: "running", StartTime: time.Now(),
		LogFile: writeTempLog(t, "gone.log", "some output\n"),
	}
	survivor := &session.Session{
		ID: "44444444-4444-4444-4444-444444444444", CLI: "codex", Model: config.GPT56SolModel,
		Mode: "review", Status: "running", StartTime: time.Now().Add(-time.Minute),
		LogFile: writeTempLog(t, "survivor.log", "other output\n"),
	}

	m := New()
	t.Cleanup(m.cancel)
	updated, _ := m.Update(SessionEvent{Sessions: []*session.Session{gone, survivor}})
	m = updated.(Model)
	updated, _ = m.Update(tea.WindowSizeMsg{Width: 80, Height: 24})
	m = updated.(Model)
	updated, _ = m.Update(press("enter"))
	m = updated.(Model)

	updated, _ = m.Update(SessionEvent{Sessions: []*session.Session{survivor}})
	m = updated.(Model)

	if m.mode != modeList {
		t.Fatal("a vanished session left the user in a detail view of someone else's run")
	}
}

// previewFixture is two finished runs and one running run, each with a log
// that names it, so a test can tell which run the preview shows.
func previewFixture(t *testing.T) []*session.Session {
	t.Helper()
	now := time.Now()
	return []*session.Session{
		{ID: "a0000000-live", CLI: "codex", Model: "gpt-6-astra", Mode: "review", Effort: "xhigh", Status: "running",
			StartTime: now.Add(-time.Minute), PID: 101, WorkDir: "/src/orbit-web",
			LogFile: writeTempLog(t, "live.log", "LIVE-RUN-OUTPUT\n")},
		{ID: "b0000000-done", CLI: "claude", Model: "claude-opus-5-5", Mode: "plan", Effort: "medium", Status: "completed",
			StartTime: now.Add(-2 * time.Minute), Duration: "2m36s", WorkDir: "/src/ledger",
			LogFile: writeTempLog(t, "done.log", "DONE-RUN-OUTPUT\n")},
		{ID: "c0000000-fail", CLI: "codex", Model: "gpt-5.5", Mode: "review", Effort: "high", Status: "failed",
			StartTime: now.Add(-3 * time.Minute), Duration: "23s", WorkDir: "/src/mathquest",
			LogFile: writeTempLog(t, "fail.log", "FAIL-RUN-OUTPUT\n")},
	}
}

func TestWideFrameShowsListAndPreviewSideBySide(t *testing.T) {
	m := newListModel(t, previewFixture(t), 200, 50)
	frame := m.viewContent()
	lines := strings.Split(frame, "\n")
	if len(lines) != 50 {
		t.Fatalf("frame has %d lines, want 50", len(lines))
	}
	assertFrameWidth(t, m, "wide list + preview")

	bodyTop := m.lay.HeaderH + 1
	top := ansi.Strip(lines[bodyTop])
	if strings.Count(top, "╭") != 2 || strings.Count(top, "╮") != 2 {
		t.Fatalf("body top %q is not two rounded boxes side by side", top)
	}
	if got := ansi.StringWidth(top); got != 200 {
		t.Fatalf("body top is %d cells, want 200", got)
	}
	// The second box starts right after the list box and the 1-col gap.
	if i := strings.Index(top, "╭"); i != 0 {
		t.Fatalf("list box starts at %d", i)
	}
	if got := []rune(top)[m.lay.ListW+1]; got != '╭' {
		t.Fatalf("preview box does not start at col %d: %q", m.lay.ListW+1, top)
	}

	row := ansi.Strip(lines[bodyTop+2])
	if !strings.Contains(row, "gpt-6-astra") {
		t.Fatalf("list column title/row missing: %q", row)
	}
	body := ansi.Strip(strings.Join(lines[bodyTop:], "\n"))
	for _, want := range []string{"orbit-web · review · codex", "output (tail)", "LIVE-RUN-OUTPUT"} {
		if !strings.Contains(body, want) {
			t.Fatalf("preview lacks %q:\n%s", want, body)
		}
	}
}

func TestFocusedPaneBorderUsesAccent(t *testing.T) {
	m := newListModel(t, previewFixture(t), 200, 50)
	top := strings.Split(m.viewContent(), "\n")[m.lay.HeaderH+1]
	accent := lipgloss.NewStyle().Foreground(colAccent).Render("╭")
	dim := lipgloss.NewStyle().Foreground(colDim).Render("╭")
	if !strings.HasPrefix(top, accent[:strings.Index(accent, "╭")]) {
		t.Fatalf("list border is not accent: %q", top)
	}
	if !strings.Contains(top, dim[:strings.Index(dim, "╭")]) {
		t.Fatalf("preview border is not dim: %q", top)
	}
}

func TestNarrowFrameHasNoPreview(t *testing.T) {
	m := newListModel(t, previewFixture(t), 100, 40)
	assertFrameWidth(t, m, "narrow")
	frame := ansi.Strip(m.viewContent())
	if strings.Contains(frame, "output (tail)") || strings.Contains(frame, "╭") {
		t.Fatalf("100 cols still shows the preview:\n%s", frame)
	}
	if n := strings.Count(frame, "\n") + 1; n != 40 {
		t.Fatalf("frame has %d lines, want 40", n)
	}
}

func TestTooSmallFrameIsTheNoticeOnly(t *testing.T) {
	m := newListModel(t, previewFixture(t), 50, 20)
	if got := m.viewContent(); got != "terminal too small (need 60×16)" {
		t.Fatalf("frame = %q", got)
	}
}

func TestMovingTheCursorRefreshesThePreview(t *testing.T) {
	m := newListModel(t, previewFixture(t), 200, 50)
	if !strings.Contains(m.viewContent(), "LIVE-RUN-OUTPUT") {
		t.Fatal("preview does not show the first run")
	}
	m = send(t, m, press("j"))
	frame := m.viewContent()
	if !strings.Contains(frame, "DONE-RUN-OUTPUT") || strings.Contains(frame, "LIVE-RUN-OUTPUT") {
		t.Fatalf("j did not move the preview to the next run:\n%s", ansi.Strip(frame))
	}
	m = send(t, m, upperKey('G'))
	if !strings.Contains(m.viewContent(), "FAIL-RUN-OUTPUT") {
		t.Fatal("G did not move the preview to the last run")
	}
}

// A tick re-reads the selected run's log only when it grew; an unchanged log,
// running or finished, costs no read.
func TestTickRereadsThePreviewOnlyWhenTheLogChanged(t *testing.T) {
	reads := 0
	orig := readTail
	readTail = func(path string, maxBytes int64) ([]byte, bool, error) {
		reads++
		return orig(path, maxBytes)
	}
	t.Cleanup(func() { readTail = orig })

	sessions := previewFixture(t)
	m := newListModel(t, sessions, 200, 50)

	// Selected run is running and its log is idle: the tick is free.
	before := reads
	m = send(t, m, tickMsg(time.Now()))
	if reads != before {
		t.Fatalf("tick on an idle running run: %d reads, want 0", reads-before)
	}
	// Its log grows: the next tick re-reads it once.
	appendToLog(t, sessions[0].LogFile, "LIVE-MORE")
	m = send(t, m, tickMsg(time.Now()))
	if reads != before+1 || !strings.Contains(m.viewContent(), "LIVE-MORE") {
		t.Fatalf("tick on a grown running log: %d reads, want 1", reads-before)
	}

	// Finished run selected: ticks (kept alive by the other running run) are
	// free.
	m = send(t, m, press("j"))
	before = reads
	send(t, m, tickMsg(time.Now()), tickMsg(time.Now()))
	if reads != before {
		t.Fatalf("tick on a finished run re-read the log %d times", reads-before)
	}
}

func TestSessionEventRefreshesAFinishedPreview(t *testing.T) {
	sessions := previewFixture(t)
	m := newListModel(t, sessions, 200, 50)
	m = send(t, m, press("j")) // the completed run
	if err := os.WriteFile(sessions[1].LogFile, []byte("REWRITTEN\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	m = send(t, m, SessionEvent{Sessions: sessions})
	if !strings.Contains(m.viewContent(), "REWRITTEN") {
		t.Fatal("SessionEvent did not re-read the selected run's log")
	}
}
