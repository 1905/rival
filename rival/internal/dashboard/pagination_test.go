package dashboard

import (
	"fmt"
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/1905/rival/internal/session"
	"github.com/charmbracelet/x/ansi"
)

// manyRuns is n runs, newest first, with ids r000, r001, ...: the first 30
// today, the next 40 yesterday, the rest older. Every third run failed.
func manyRuns(n int, now time.Time) []*session.Session {
	out := make([]*session.Session, 0, n)
	for i := 0; i < n; i++ {
		var start time.Time
		switch {
		case i < 30:
			start = now.Add(-time.Duration(i) * time.Millisecond)
		case i < 70:
			start = now.AddDate(0, 0, -1)
		default:
			start = now.AddDate(0, 0, -40)
		}
		status := "completed"
		if i%3 == 0 {
			status = "failed"
		}
		out = append(out, &session.Session{
			ID: fmt.Sprintf("r%03d", i), CLI: "codex", Model: "gpt-5.5", Mode: "review",
			Effort: "high", Status: status, StartTime: start, Duration: "1m", WorkDir: "/src/proj",
		})
	}
	return out
}

func manyListPane(n int, now time.Time) listPane {
	l := newListPane()
	l.setItems(groupSessions(manyRuns(n, now)), now)
	l.top()
	return l
}

// pageSummary is the current page's rows: "#SECTION" or the run id.
func pageSummary(l *listPane) []string {
	rows, _ := l.pageRows()
	var out []string
	for _, r := range rows {
		if r.Item == nil {
			out = append(out, "#"+r.Section)
			continue
		}
		out = append(out, r.Item.Primary().ID)
	}
	return out
}

func TestPageSlicingWithSections(t *testing.T) {
	now := noonToday()
	l := manyListPane(120, now)
	if got := l.pageCount(); got != 3 {
		t.Fatalf("120 runs: %d pages, want 3", got)
	}

	type want struct {
		first, last string
		headers     []string
		runs        int
	}
	pages := []want{
		{"r000", "r049", []string{"#TODAY", "#YESTERDAY"}, 50},
		// Page 2 starts mid-YESTERDAY: it gets its own copy of the header.
		{"r050", "r099", []string{"#YESTERDAY", "#OLDER"}, 50},
		{"r100", "r119", []string{"#OLDER"}, 20},
	}
	for p, w := range pages {
		if l.page() != p {
			t.Fatalf("page() = %d, want %d", l.page(), p)
		}
		rows := pageSummary(&l)
		var headers, runs []string
		for _, r := range rows {
			if strings.HasPrefix(r, "#") {
				headers = append(headers, r)
			} else {
				runs = append(runs, r)
			}
		}
		if !strings.HasPrefix(rows[0], "#") {
			t.Fatalf("page %d does not start with a section header: %v", p+1, rows[:3])
		}
		if fmt.Sprint(headers) != fmt.Sprint(w.headers) || len(runs) != w.runs || runs[0] != w.first || runs[len(runs)-1] != w.last {
			t.Fatalf("page %d: headers %v, %d runs %s..%s; want %v, %d runs %s..%s",
				p+1, headers, len(runs), runs[0], runs[len(runs)-1], w.headers, w.runs, w.first, w.last)
		}
		if sel := l.selected().Primary().ID; sel != w.first {
			t.Fatalf("page %d: cursor on %s, want the first run %s", p+1, sel, w.first)
		}
		l.turnPage(1)
	}

	for n, pages := range map[int]int{0: 1, 1: 1, 50: 1, 51: 2, 100: 2, 101: 3} {
		l := manyListPane(n, now)
		if got := l.pageCount(); got != pages {
			t.Errorf("%d runs: %d pages, want %d", n, got, pages)
		}
	}
}

func TestPageKeysStopAtTheEnds(t *testing.T) {
	m := newListModel(t, manyRuns(120, time.Now()), 120, 40)
	m = send(t, m, press("p"))
	if m.list.page() != 0 || selectedID(m) != "r000" {
		t.Fatalf("p on page 1: page %d, selected %s", m.list.page()+1, selectedID(m))
	}
	m = send(t, m, press("n"))
	if m.list.page() != 1 || selectedID(m) != "r050" {
		t.Fatalf("n: page %d, selected %s", m.list.page()+1, selectedID(m))
	}
	m = send(t, m, press("n"), press("n"), press("n"))
	if m.list.page() != 2 || selectedID(m) != "r100" {
		t.Fatalf("n past the end: page %d, selected %s", m.list.page()+1, selectedID(m))
	}
	m = send(t, m, press("pgup"))
	if m.list.page() != 1 || selectedID(m) != "r050" {
		t.Fatalf("pgup: page %d, selected %s", m.list.page()+1, selectedID(m))
	}
	m = send(t, m, press("pgdown"))
	if m.list.page() != 2 {
		t.Fatalf("pgdown: page %d", m.list.page()+1)
	}
	m = send(t, m, press("p"), press("p"), press("p"))
	if m.list.page() != 0 || selectedID(m) != "r000" {
		t.Fatalf("p past the start: page %d, selected %s", m.list.page()+1, selectedID(m))
	}
}

func TestJKCrossPageEdgesAndGJumpAcrossPages(t *testing.T) {
	m := newListModel(t, manyRuns(120, time.Now()), 120, 40)
	m = send(t, m, press("n"), press("k"))
	if m.list.page() != 0 || selectedID(m) != "r049" {
		t.Fatalf("k from the top of page 2: page %d, selected %s", m.list.page()+1, selectedID(m))
	}
	m = send(t, m, press("j"))
	if m.list.page() != 1 || selectedID(m) != "r050" {
		t.Fatalf("j from the bottom of page 1: page %d, selected %s", m.list.page()+1, selectedID(m))
	}
	m = send(t, m, upperKey('G'))
	if m.list.page() != 2 || selectedID(m) != "r119" {
		t.Fatalf("G: page %d, selected %s", m.list.page()+1, selectedID(m))
	}
	m = send(t, m, press("j"))
	if selectedID(m) != "r119" {
		t.Fatalf("j past the last run: %s", selectedID(m))
	}
	m = send(t, m, press("g"))
	if m.list.page() != 0 || selectedID(m) != "r000" {
		t.Fatalf("g: page %d, selected %s", m.list.page()+1, selectedID(m))
	}
	// The selected run is always on screen, whatever the page.
	m = send(t, m, upperKey('G'))
	if !strings.Contains(ansi.Strip(m.viewContent()), "page 3/3") {
		t.Fatal("G did not show page 3")
	}
}

func TestTabAndFilterChangesResetToPageOne(t *testing.T) {
	m := newListModel(t, manyRuns(200, time.Now()), 120, 40)
	m = send(t, m, press("n"), press("n"), press("j"))
	if m.list.page() != 2 {
		t.Fatalf("setup: page %d", m.list.page()+1)
	}
	m = send(t, m, press("tab")) // RUNNING: empty
	m = send(t, m, press("tab")) // FAILED: 67 runs, 2 pages
	if m.list.tab != tabFailed || m.list.page() != 0 || selectedID(m) != "r000" {
		t.Fatalf("tab: tab %v page %d selected %s", m.list.tab, m.list.page()+1, selectedID(m))
	}
	m = send(t, m, press("n"))
	if m.list.page() != 1 {
		t.Fatalf("n on FAILED: page %d", m.list.page()+1)
	}
	m = send(t, m, press("shift+tab"), press("shift+tab"))
	if m.list.tab != tabAll || m.list.page() != 0 || selectedID(m) != "r000" {
		t.Fatalf("shift+tab: tab %v page %d selected %s", m.list.tab, m.list.page()+1, selectedID(m))
	}

	// A filter change resets too, and n/p typed into the filter stay text.
	m = send(t, m, press("n"), press("n"))
	m = send(t, m, press("/"))
	m = send(t, m, typeText("np")...)
	if m.mode != modeFilter || m.list.filter.Value() != "np" {
		t.Fatalf("n/p in the filter: mode %d, value %q", m.mode, m.list.filter.Value())
	}
	m = send(t, m, press("esc"))
	if m.list.page() != 0 || selectedID(m) != "r000" {
		t.Fatalf("filter change: page %d, selected %s", m.list.page()+1, selectedID(m))
	}
	m = send(t, m, press("n"), press("/"))
	m = send(t, m, typeText("r1")...)
	if m.list.page() != 0 || selectedID(m) != "r100" {
		t.Fatalf("typing a filter: page %d, selected %s", m.list.page()+1, selectedID(m))
	}
}

func TestRefreshKeepsTheSelectedRunAndItsPage(t *testing.T) {
	now := time.Now()
	sessions := manyRuns(120, now)
	m := newListModel(t, sessions, 120, 40)
	m = send(t, m, press("n"))
	for i := 0; i < 25; i++ {
		m = send(t, m, press("j"))
	}
	if selectedID(m) != "r075" || m.list.page() != 1 {
		t.Fatalf("setup: %s on page %d", selectedID(m), m.list.page()+1)
	}

	// 30 new runs above push r075 to ordinal 105: page 3.
	var fresh []*session.Session
	for i := 0; i < 30; i++ {
		fresh = append(fresh, &session.Session{ID: fmt.Sprintf("n%03d", i), CLI: "codex", Status: "completed", StartTime: now.Add(time.Duration(i+1) * time.Millisecond)})
	}
	m = send(t, m, SessionEvent{Sessions: append(fresh, sessions...)})
	if selectedID(m) != "r075" || m.list.page() != 2 {
		t.Fatalf("after insert above: %s on page %d, want r075 on page 3", selectedID(m), m.list.page()+1)
	}
	if !strings.Contains(ansi.Strip(m.viewContent()), "page 3/3") {
		t.Fatal("the frame does not show page 3")
	}

	// The run vanishes and the list shrinks to 2 pages: the page clamps.
	m = send(t, m, SessionEvent{Sessions: sessions[:60]})
	if m.list.selected() == nil || m.list.page() != 1 {
		t.Fatalf("after the run vanished: selected %q on page %d, want the last page", selectedID(m), m.list.page()+1)
	}
}

func TestPageFooterText(t *testing.T) {
	now := noonToday()
	l := manyListPane(120, now)
	if got := l.footerText(); got != "‹ prev  page 1/3  next ›  · 120 runs" {
		t.Fatalf("footer = %q", got)
	}
	l.turnPage(1)
	if got := l.footerText(); got != "‹ prev  page 2/3  next ›  · 120 runs" {
		t.Fatalf("footer = %q", got)
	}
	small := manyListPane(4, now)
	if got := small.footerText(); got != "4 runs" {
		t.Fatalf("one-page footer = %q", got)
	}
	empty := manyListPane(0, now)
	if got := empty.footerText(); got != "0 runs" {
		t.Fatalf("empty footer = %q", got)
	}
	// prev is dim on page 1, next is dim on the last page.
	first := manyListPane(120, now)
	if a, b := first.footer(80), dimStyle.Render("‹ prev"); !strings.Contains(a, b) {
		t.Fatalf("prev is not dim on page 1: %q", a)
	}
	first.turnPage(5)
	if a, b := first.footer(80), dimStyle.Render("next ›"); !strings.Contains(a, b) {
		t.Fatalf("next is not dim on the last page: %q", a)
	}
	// It always fills the width exactly, even when it has to be cut.
	for _, w := range []int{10, 30, 66, 200} {
		if got := ansi.StringWidth(l.footer(w)); got != w {
			t.Fatalf("footer at %d cells is %d wide", w, got)
		}
	}
}

// The footer sits on the last line of the list pane, in the narrow and the
// split layout.
func TestFooterIsTheLastLineOfTheListPane(t *testing.T) {
	for _, width := range []int{90, 200} {
		m := newListModel(t, manyRuns(120, time.Now()), width, 40)
		m = send(t, m, press("n"))
		list := strings.Split(m.list.view(m.listInnerWidth(), m.listInnerHeight(), ""), "\n")
		if got := ansi.Strip(list[len(list)-1]); !strings.Contains(got, "page 2/3") {
			t.Fatalf("width %d: last list line %q is not the footer", width, got)
		}
	}
}

// Detail mode keeps n for search matches: it never turns the list page.
func TestDetailNDoesNotTurnThePage(t *testing.T) {
	m := newListModel(t, manyRuns(120, time.Now()), 120, 40)
	m = send(t, m, press("enter"), press("n"))
	if m.mode != modeDetail || m.list.page() != 0 || selectedID(m) != "r000" {
		t.Fatalf("n in detail: mode %d page %d selected %s", m.mode, m.list.page()+1, selectedID(m))
	}
}

// The paginated frame fills the terminal exactly: every line is exactly the
// terminal width and there are exactly height lines, on every page, with the
// help short or expanded, even when the pane is shorter than a page.
func TestPaginatedFrameFitsTheTerminal(t *testing.T) {
	for _, width := range []int{60, 90, 120, 200} {
		for _, height := range []int{16, 30, 50} {
			m := newListModel(t, manyRuns(120, time.Now()), width, height)
			for _, step := range []tea.Msg{nil, press("n"), press("j"), upperKey('G'), press("k"), press("?"), press("p"), press("g")} {
				if step != nil {
					m = send(t, m, step)
				}
				label := fmt.Sprintf("%dx%d after %v", width, height, step)
				lines := strings.Split(m.viewContent(), "\n")
				if len(lines) != height {
					t.Fatalf("%s: frame has %d lines, want %d", label, len(lines), height)
				}
				assertFrameWidth(t, m, label)
				// The tab bar and the list pane are exactly the width; the
				// help bar is only bounded by it.
				bodyEnd := m.lay.HeaderH + 1 + m.listBodyHeight()
				for i := m.lay.HeaderH; i < bodyEnd; i++ {
					if w := ansi.StringWidth(lines[i]); w != width {
						t.Fatalf("%s: line %d is %d cells, want %d: %q", label, i, w, width, ansi.Strip(lines[i]))
					}
				}
				footerLine := bodyEnd - 1
				if m.lay.ShowPreview {
					footerLine-- // the bottom border is below it
				}
				if got := ansi.Strip(lines[footerLine]); !strings.Contains(got, fmt.Sprintf("page %d/3", m.list.page()+1)) {
					t.Fatalf("%s: the list pane's last line is not the footer: %q", label, got)
				}
				// The cursor is inside the scrolled window of its page.
				if _, cur := m.list.pageRows(); cur < m.list.offset || cur >= m.list.offset+m.listRows() {
					t.Fatalf("%s: cursor %d outside the window %d+%d", label, cur, m.list.offset, m.listRows())
				}
			}
		}
	}
}

func TestRunCountPlural(t *testing.T) {
	for n, want := range map[int]string{0: "0 runs", 1: "1 run", 2: "2 runs", 120: "120 runs"} {
		if got := runCount(n); got != want {
			t.Errorf("runCount(%d) = %q, want %q", n, got, want)
		}
	}
}
