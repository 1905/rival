package dashboard

import (
	"fmt"
	"os"
	"strings"
	"syscall"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/1905/rival/internal/session"
	"github.com/charmbracelet/x/ansi"
)

// TestMain points HOME at a throwaway directory for the whole package. Opening
// the detail screen calls session.Load for the full prompt, and without this
// a fixture id would be looked up in the real ~/.rival.
func TestMain(m *testing.M) {
	home, err := os.MkdirTemp("", "rival-dashboard-test-home-*")
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	_ = os.Setenv("HOME", home)
	code := m.Run()
	_ = os.RemoveAll(home)
	os.Exit(code)
}

// groupFixture is a two-reviewer group plus its judge, handed over in the
// wrong order so grouping has to sort it.
func groupFixture(t *testing.T) []*session.Session {
	t.Helper()
	base := time.Now().Add(-5 * time.Minute)
	q1, q2, q3 := base, base.Add(time.Second), base.Add(2*time.Second)
	return []*session.Session{
		{ID: "judge000-0000", GroupID: "grp00000-0000", CLI: "codex", Model: "gpt-6-astra", Mode: "consilium", Status: "running", StartTime: q3, QueuedAt: &q3, PID: 901, WorkDir: "/src/orbit-web",
			LogFile: writeTempLog(t, "judge.log", "JUDGE-OUTPUT\nVERDICT: 6/10\n")},
		{ID: "gemini00-0000", GroupID: "grp00000-0000", CLI: "opencode", Model: "gemini-3.1", Mode: "megareview", Status: "completed", StartTime: q2, QueuedAt: &q2, WorkDir: "/src/orbit-web",
			LogFile: writeTempLog(t, "gemini.log", "GEMINI-OUTPUT\n")},
		{ID: "gpt55000-0000", GroupID: "grp00000-0000", CLI: "codex", Model: "gpt-5.5", Mode: "megareview", Status: "running", StartTime: q1, QueuedAt: &q1, PID: 902, WorkDir: "/src/orbit-web",
			LogFile: writeTempLog(t, "gpt55.log", "GPT55-OUTPUT\n")},
	}
}

// openDetail builds a model at width×height with sessions and opens the
// first run through the real enter key.
func openDetail(t *testing.T, sessions []*session.Session, width, height int) Model {
	t.Helper()
	m := newListModel(t, sessions, width, height)
	m = send(t, m, press("enter"))
	if m.mode != modeDetail {
		t.Fatal("enter did not open the detail screen")
	}
	return m
}

func frameText(m Model) string { return ansi.Strip(m.viewContent()) }

func TestDetailOpenDefaults(t *testing.T) {
	m := openDetail(t, groupFixture(t), 120, 40)
	if m.detail.tab != tabOutput || m.detail.memberIndex(m.list.selected()) != 0 || !m.detail.follow {
		t.Fatalf("open: tab %d member %d follow %v, want Output, 0, true", m.detail.tab, m.detail.memberIndex(m.list.selected()), m.detail.follow)
	}
}

func TestDetailTabKeys(t *testing.T) {
	m := openDetail(t, previewFixture(t), 120, 40)
	for _, tc := range []struct {
		key  tea.Msg
		want detailTab
	}{
		{press("2"), tabPrompt}, {press("3"), tabInfo}, {press("1"), tabOutput},
		{press("tab"), tabPrompt}, {press("tab"), tabInfo}, {press("tab"), tabOutput},
		{press("shift+tab"), tabInfo},
	} {
		m = send(t, m, tc.key)
		if m.detail.tab != tc.want {
			t.Fatalf("after %v: tab %d, want %d", tc.key, m.detail.tab, tc.want)
		}
	}
	if !strings.Contains(frameText(m), "[ Info ]") {
		t.Fatalf("tab bar does not mark Info active:\n%s", frameText(m))
	}
}

func TestDetailMembersFollowSortGroupMembersAndWrap(t *testing.T) {
	sessions := groupFixture(t)
	m := openDetail(t, sessions, 160, 40)
	item := m.list.selected()

	want := append([]*session.Session(nil), sessions...)
	session.SortGroupMembers(want)
	if want[len(want)-1].Mode != "consilium" {
		t.Fatal("test premise broken: judge does not sort last")
	}
	for i, s := range want {
		if item.Sessions[i].ID != s.ID {
			t.Fatalf("member %d = %s, want %s", i, item.Sessions[i].ID, s.ID)
		}
	}

	bar := strings.Split(frameText(m), "\n")[m.lay.HeaderH+1]
	for _, label := range []string{"[ gpt-5.5 ]", "gemini-3.1", "judge"} {
		if !strings.Contains(bar, label) {
			t.Fatalf("member bar %q lacks %q", bar, label)
		}
	}
	if strings.Contains(bar, "gpt-6-astra") {
		t.Fatalf("judge shown by model id instead of \"judge\": %q", bar)
	}

	if !strings.Contains(frameText(m), "GPT55-OUTPUT") {
		t.Fatal("member 0 output not shown")
	}
	m = send(t, m, press("]"))
	if m.detail.memberIndex(m.list.selected()) != 1 || !strings.Contains(frameText(m), "GEMINI-OUTPUT") {
		t.Fatalf("] -> member %d", m.detail.memberIndex(m.list.selected()))
	}
	m = send(t, m, press("]"), press("]"))
	if m.detail.memberIndex(m.list.selected()) != 0 {
		t.Fatalf("] did not wrap to 0: %d", m.detail.memberIndex(m.list.selected()))
	}
	m = send(t, m, press("["))
	if m.detail.memberIndex(m.list.selected()) != 2 || !strings.Contains(frameText(m), "JUDGE-OUTPUT") {
		t.Fatalf("[ did not wrap to the judge: %d", m.detail.memberIndex(m.list.selected()))
	}
}

func TestDetailPromptTab(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	stored, err := session.NewQueued("codex", "review", "gpt-6-astra", "high", "/src/proj", "FULL-PROMPT "+strings.Repeat("word ", 60)+"THE-END", "", "")
	if err != nil {
		t.Fatal(err)
	}
	summary := *stored
	summary.Prompt = "" // the list holds summaries
	summary.PID = 0
	missing := &session.Session{ID: "missing0-0000", CLI: "codex", Model: "gpt-5.5", Status: "completed",
		StartTime: time.Now().Add(-time.Hour), PromptPreview: "PREVIEW-ONLY"}

	m := openDetail(t, []*session.Session{&summary, missing}, 100, 40)
	m = send(t, m, press("2"))
	got := strings.Join(m.detail.lines, "\n")
	if !strings.Contains(got, "FULL-PROMPT") || !strings.Contains(got, "THE-END") {
		t.Fatalf("prompt tab lacks the full stored prompt:\n%s", got)
	}
	for _, l := range m.detail.lines {
		if w := ansi.StringWidth(l); w > 100 {
			t.Fatalf("prompt line %d cells wide", w)
		}
	}

	m = send(t, m, press("esc"), press("j"), press("enter"), press("2"))
	got = ansi.Strip(strings.Join(m.detail.lines, "\n"))
	if !strings.Contains(got, "PREVIEW-ONLY") || !strings.Contains(got, "(full prompt unavailable)") {
		t.Fatalf("unreadable prompt did not fall back to the preview:\n%s", got)
	}
}

func TestInfoLinesListEveryField(t *testing.T) {
	start := time.Date(2026, 9, 26, 11, 40, 0, 0, time.Local)
	end := start.Add(90 * time.Second)
	queued := start.Add(-time.Minute)
	exit := 1
	longErr := "provider exploded: " + strings.Repeat("detail ", 40) + "TAIL_OF_ERROR"
	s := &session.Session{
		ID: "abcdef01-2345", GroupID: "group123", CLI: "codex", Model: "gpt-6-astra", Effort: "xhigh",
		Mode: "review", Status: "failed", ExitCode: &exit, StartTime: start, EndTime: &end, Duration: "1m30s",
		QueuedAt: &queued, WorkDir: "/src/orbit-web", ReviewScope: "plans/x.md", Account: "work",
		PID: 81233, OutputBytes: 2048, OutputLines: 17, LogFile: "/tmp/x.log", ErrorMsg: longErr,
	}
	lines := infoLines(s, 80)
	got := ansi.Strip(strings.Join(lines, "\n"))
	for _, want := range []string{
		"id", "abcdef01-2345", "group id", "group123", "cli", "codex", "model", "gpt-6-astra",
		"effort", "xhigh", "mode", "review", "status", "failed", "exit", "started", "2026-09-26 11:40:00",
		"ended", "2026-09-26 11:41:30", "duration", "1m30s", "queued at", "2026-09-26 11:39:00",
		"workdir", "/src/orbit-web", "scope", "plans/x.md", "account", "work", "pid", "81233",
		"output", "2048 bytes, 17 lines", "log", "/tmp/x.log", "TAIL_OF_ERROR",
	} {
		if !strings.Contains(got, want) {
			t.Errorf("info lacks %q:\n%s", want, got)
		}
	}
	for _, l := range lines {
		if w := ansi.StringWidth(l); w > 80 {
			t.Fatalf("info line %d cells wide: %q", w, l)
		}
	}
}

func TestDetailFollowPausesAndResumes(t *testing.T) {
	logPath := writeTempLog(t, "follow.log", strings.Repeat("initial line\n", 200))
	s := &session.Session{ID: "follow00-0000", CLI: "codex", Model: "gpt-6-astra", Mode: "review",
		Status: "running", StartTime: time.Now(), LogFile: logPath}
	m := openDetail(t, []*session.Session{s}, 80, 30)
	if !m.detail.vp.AtBottom() || !m.detail.follow {
		t.Fatal("detail did not open at the tail with follow on")
	}
	if !strings.Contains(frameText(m), "follow ●") {
		t.Fatalf("breadcrumb lacks follow ●:\n%s", frameText(m))
	}

	appendLine := func(marker string) {
		t.Helper()
		f, err := os.OpenFile(logPath, os.O_APPEND|os.O_WRONLY, 0o600)
		if err != nil {
			t.Fatal(err)
		}
		_, _ = f.WriteString(marker + "\n")
		_ = f.Close()
	}

	appendLine("MARKER-ONE")
	m = send(t, m, tickMsg(time.Now()))
	if !strings.Contains(m.detail.vp.GetContent(), "MARKER-ONE") || !m.detail.vp.AtBottom() {
		t.Fatal("tick while following did not keep the new tail in view")
	}

	m = send(t, m, press("k"))
	if m.detail.follow || m.detail.vp.AtBottom() {
		t.Fatal("scrolling up did not pause follow")
	}
	if !strings.Contains(frameText(m), "follow ○") {
		t.Fatalf("breadcrumb lacks follow ○:\n%s", frameText(m))
	}
	offset := m.detail.vp.YOffset()
	appendLine("MARKER-TWO")
	m = send(t, m, tickMsg(time.Now()))
	if m.detail.vp.YOffset() != offset || m.detail.vp.AtBottom() {
		t.Fatal("a tick moved a reader who had scrolled up")
	}
	if !strings.Contains(m.detail.vp.GetContent(), "MARKER-TWO") {
		t.Fatal("tick did not re-read the log while paused")
	}

	m = send(t, m, upperKey('G'))
	if !m.detail.follow || !m.detail.vp.AtBottom() {
		t.Fatal("G did not resume follow at the bottom")
	}
	m = send(t, m, press("k"), press("f"))
	if !m.detail.follow || !m.detail.vp.AtBottom() {
		t.Fatal("f did not resume follow at the bottom")
	}

	// Capture-before-resize regression: shrinking while following must keep
	// the tail, and so must the help bar growing.
	m = send(t, m, tea.WindowSizeMsg{Width: 80, Height: 18})
	if !m.detail.vp.AtBottom() {
		t.Fatal("resize while following lost the tail")
	}
	m = send(t, m, press("?"))
	appendLine("MARKER-THREE")
	m = send(t, m, SessionEvent{Sessions: []*session.Session{s}})
	if !m.detail.vp.AtBottom() || !strings.Contains(m.detail.vp.GetContent(), "MARKER-THREE") {
		t.Fatal("SessionEvent while following did not keep the tail")
	}
}

func TestDetailBreadcrumb(t *testing.T) {
	m := openDetail(t, previewFixture(t), 120, 40)
	crumb := strings.Split(frameText(m), "\n")[m.lay.HeaderH]
	for _, want := range []string{"rival › orbit-web › review a0000000", "running", "follow ●"} {
		if !strings.Contains(crumb, want) {
			t.Fatalf("breadcrumb %q lacks %q", crumb, want)
		}
	}
	m = send(t, m, press("esc"), press("j"), press("enter"))
	crumb = strings.Split(frameText(m), "\n")[m.lay.HeaderH]
	for _, want := range []string{"ledger › plan b0000000", "✓ completed 2m36s"} {
		if !strings.Contains(crumb, want) {
			t.Fatalf("breadcrumb %q lacks %q", crumb, want)
		}
	}
}

func TestDetailTooSmallTerminal(t *testing.T) {
	m := openDetail(t, previewFixture(t), 80, 30)
	m = send(t, m, tea.WindowSizeMsg{Width: 80, Height: 10})
	if got := m.viewContent(); got != tooSmallNotice {
		t.Fatalf("tiny detail frame = %q", got)
	}
	if h := m.detail.vp.Height(); h < 1 {
		t.Fatalf("viewport height %d, want >= 1", h)
	}
}

func TestFindMatches(t *testing.T) {
	lines := []string{"alpha", "Fingerprint here", "none", "\x1b[31mFINGER\x1b[0mprint", "fingerprint again"}
	got := findMatches(lines, "fingerPRINT")
	if fmt.Sprint(got) != "[1 3 4]" {
		t.Fatalf("findMatches = %v, want [1 3 4]", got)
	}
	if findMatches(lines, "") != nil {
		t.Fatal("empty query matched")
	}
}

func searchFixture(t *testing.T) []*session.Session {
	t.Helper()
	var b strings.Builder
	for i := range 120 {
		switch i {
		case 10, 50, 90:
			fmt.Fprintf(&b, "line %d has a Fingerprint in it\n", i)
		default:
			fmt.Fprintf(&b, "line %d plain\n", i)
		}
	}
	return []*session.Session{{ID: "search00-0000", CLI: "codex", Model: "gpt-6-astra", Mode: "review",
		Status: "completed", StartTime: time.Now(), Duration: "1m", LogFile: writeTempLog(t, "search.log", b.String())}}
}

func visible(m Model, line int) bool {
	off := m.detail.vp.YOffset()
	return line >= off && line < off+m.detail.vp.Height()
}

func TestDetailSearch(t *testing.T) {
	m := openDetail(t, searchFixture(t), 100, 30)
	m = send(t, m, press("/"))
	if m.mode != modeSearch || !m.detail.search.Focused() {
		t.Fatal("/ did not focus the search input")
	}
	m = send(t, m, typeText("fingerprint")...)
	m = send(t, m, press("enter"))
	if m.mode != modeDetail || m.detail.query != "fingerprint" {
		t.Fatalf("enter: mode %d query %q", m.mode, m.detail.query)
	}
	if fmt.Sprint(m.detail.matches) != "[10 50 90]" {
		t.Fatalf("matches = %v", m.detail.matches)
	}
	if !visible(m, 10) {
		t.Fatalf("first match line 10 not visible at offset %d", m.detail.vp.YOffset())
	}
	if m.detail.follow {
		t.Fatal("jumping to a match left follow on")
	}
	if !strings.Contains(frameText(m), "1/3 matches") {
		t.Fatalf("status line lacks 1/3 matches:\n%s", frameText(m))
	}

	m = send(t, m, press("n"))
	if !visible(m, 50) || !strings.Contains(frameText(m), "2/3 matches") {
		t.Fatal("n did not move to match 2")
	}
	m = send(t, m, press("n"), press("n"))
	if !visible(m, 10) || !strings.Contains(frameText(m), "1/3 matches") {
		t.Fatal("n did not wrap to match 1")
	}
	m = send(t, m, upperKey('N'))
	if !visible(m, 90) || !strings.Contains(frameText(m), "3/3 matches") {
		t.Fatal("N did not wrap back to match 3")
	}

	// The hit is painted with the accent background, and the line keeps its
	// width.
	vpText := m.detail.vp.View()
	if !strings.Contains(vpText, matchStyle.Render("Fingerprint")) {
		t.Fatalf("match not highlighted:\n%q", vpText)
	}
	for i, l := range m.detail.lines {
		if hl := highlightLine(l, "fingerprint"); ansi.StringWidth(hl) != ansi.StringWidth(l) || ansi.Strip(hl) != ansi.Strip(l) {
			t.Fatalf("highlight changed line %d: %q -> %q", i, l, hl)
		}
	}

	m = send(t, m, press("esc"))
	if m.detail.query != "" || m.detail.matches != nil || m.mode != modeDetail {
		t.Fatal("esc did not clear the search")
	}
	if strings.Contains(m.detail.vp.View(), matchStyle.Render("Fingerprint")) {
		t.Fatal("highlight survived esc")
	}
	m = send(t, m, press("esc"))
	if m.mode != modeList {
		t.Fatal("second esc did not return to the list")
	}
}

func TestDetailSearchNoMatches(t *testing.T) {
	m := openDetail(t, searchFixture(t), 100, 30)
	m = send(t, m, press("/"))
	m = send(t, m, typeText("zzz")...)
	m = send(t, m, press("enter"))
	if !strings.Contains(frameText(m), "no matches") {
		t.Fatalf("status line lacks no matches:\n%s", frameText(m))
	}
}

func TestSearchInputTypesQ(t *testing.T) {
	m := openDetail(t, searchFixture(t), 100, 30)
	m = send(t, m, press("/"), press("q"), press("n"))
	if m.quitting {
		t.Fatal("q in the search input quit the TUI")
	}
	if got := m.detail.search.Value(); got != "qn" {
		t.Fatalf("search input = %q, want qn", got)
	}
	m = send(t, m, press("esc"))
	if m.mode != modeDetail || m.detail.search.Value() != "" {
		t.Fatal("esc in the search input did not clear it")
	}
}

// killRecorder replaces Model.kill. It never sends a real signal.
type killRecorder struct {
	pids []int
	sigs []syscall.Signal
}

func (r *killRecorder) kill(pid int, sig syscall.Signal) error {
	r.pids = append(r.pids, pid)
	r.sigs = append(r.sigs, sig)
	return nil
}

// runningGroup stores two running members in the temp HOME so
// failSessionForKill can reload and save them. Their PIDs are fake: the
// recorder is the only thing that ever sees them.
func runningGroup(t *testing.T) []*session.Session {
	t.Helper()
	t.Setenv("HOME", t.TempDir())
	var out []*session.Session
	for i, model := range []string{"gpt-5.5", "gemini-3.1"} {
		s, err := session.NewQueued("codex", "megareview", model, "high", "/src/proj", "prompt", "", "killgroup")
		if err != nil {
			t.Fatal(err)
		}
		if err := s.MarkRunning(); err != nil {
			t.Fatal(err)
		}
		s.PID = 990001 + i
		if err := s.Save(); err != nil {
			t.Fatal(err)
		}
		out = append(out, s)
	}
	return out
}

func openKillModel(t *testing.T, sessions []*session.Session) (Model, *killRecorder) {
	t.Helper()
	m := newListModel(t, sessions, 120, 40)
	rec := &killRecorder{}
	m.kill = rec.kill
	m.alive = func(int, int64) bool { return true } // fake pids stand in for live runs
	m = send(t, m, press("enter"))
	return m, rec
}

func TestStopAsksBeforeSignalling(t *testing.T) {
	m, rec := openKillModel(t, runningGroup(t))
	m = send(t, m, press("x"))
	if m.mode != modeConfirm {
		t.Fatalf("x did not open the confirm bar (mode %d)", m.mode)
	}
	if !strings.Contains(frameText(m), "stop 2 running sessions? y/n") {
		t.Fatalf("confirm bar text wrong:\n%s", frameText(m))
	}
	if len(rec.pids) != 0 {
		t.Fatalf("x alone signalled %v", rec.pids)
	}
}

func TestStopCancelSendsNothing(t *testing.T) {
	for _, k := range []string{"n", "esc", "j"} {
		m, rec := openKillModel(t, runningGroup(t))
		m = send(t, m, press("x"), press(k))
		if len(rec.pids) != 0 {
			t.Fatalf("%s after x signalled %v", k, rec.pids)
		}
		if m.mode != modeDetail || m.detail.confirm != nil {
			t.Fatalf("%s did not close the confirm bar", k)
		}
		if strings.Contains(frameText(m), "y/n") {
			t.Fatalf("%s left the bar on screen", k)
		}
	}
}

func TestStopYesSignalsAndFails(t *testing.T) {
	sessions := runningGroup(t)
	m, rec := openKillModel(t, sessions)
	m = send(t, m, press("x"), press("y"))
	if fmt.Sprint(rec.pids) != "[990001 990002]" {
		t.Fatalf("signalled %v, want both fake pids", rec.pids)
	}
	for _, sig := range rec.sigs {
		if sig != syscall.SIGTERM {
			t.Fatalf("sent %v, want SIGTERM", sig)
		}
	}
	for _, s := range sessions {
		stored, err := session.Load(s.ID)
		if err != nil {
			t.Fatal(err)
		}
		if stored.Status != "failed" || stored.ExitCode == nil || *stored.ExitCode != 137 || stored.ErrorMsg != "killed by user" {
			t.Fatalf("%s stored as %s/%v/%q, want failed/137/killed by user", s.Model, stored.Status, stored.ExitCode, stored.ErrorMsg)
		}
	}
	if m.mode != modeDetail {
		t.Fatalf("mode after y = %d", m.mode)
	}
}

func TestStopYesOnDeadProcessFailsWithExit1(t *testing.T) {
	sessions := runningGroup(t)[:1]
	m := newListModel(t, sessions, 120, 40)
	m.kill = func(int, syscall.Signal) error { return syscall.ESRCH }
	m.alive = func(int, int64) bool { return true }
	m = send(t, m, press("enter"), press("x"), press("y"))
	stored, err := session.Load(sessions[0].ID)
	if err != nil {
		t.Fatal(err)
	}
	if stored.ExitCode == nil || *stored.ExitCode != 1 || stored.ErrorMsg != "killed (process already dead)" {
		t.Fatalf("dead process stored as %v/%q", stored.ExitCode, stored.ErrorMsg)
	}
}

func TestStopOnFinishedRunSaysNothingRunning(t *testing.T) {
	m, rec := openKillModel(t, []*session.Session{{ID: "done0000-0000", CLI: "codex", Model: "gpt-5.5",
		Status: "completed", StartTime: time.Now(), Duration: "1m", PID: 990009}})
	m = send(t, m, press("x"))
	if m.mode != modeDetail || m.detail.confirm != nil {
		t.Fatal("x on a finished run opened the confirm bar")
	}
	if !strings.Contains(frameText(m), "nothing running") {
		t.Fatalf("no nothing-running notice:\n%s", frameText(m))
	}
	if len(rec.pids) != 0 {
		t.Fatalf("finished run signalled %v", rec.pids)
	}
	m = send(t, m, press("j"))
	if strings.Contains(frameText(m), "nothing running") {
		t.Fatal("notice did not clear on the next key")
	}
}

// A run that finishes while the bar is open must not be signalled: its PID
// may already belong to another process.
func TestStopYesSkipsARunThatFinishedMeanwhile(t *testing.T) {
	sessions := runningGroup(t)
	m, rec := openKillModel(t, sessions)
	m = send(t, m, press("x"))
	done := *sessions[0]
	done.Status = "completed"
	m = send(t, m, SessionEvent{Sessions: []*session.Session{&done, sessions[1]}})
	send(t, m, press("y"))
	if fmt.Sprint(rec.pids) != "[990002]" {
		t.Fatalf("signalled %v, want only the still-running 990002", rec.pids)
	}
}

// A stale "running" record whose PID now belongs to another process must not
// be signalled; it is failed as already dead instead.
func TestStopNeverSignalsAReusedPID(t *testing.T) {
	sessions := runningGroup(t)[:1]
	m, rec := openKillModel(t, sessions)
	m = send(t, m, press("x"))
	if m.mode != modeConfirm {
		t.Fatal("x on a live run did not open the confirm bar")
	}
	// The process dies, and its PID is reused, while the bar is open.
	m.alive = func(int, int64) bool { return false }
	m = send(t, m, press("y"))
	if len(rec.pids) != 0 {
		t.Fatalf("signalled reused pid(s) %v", rec.pids)
	}
	stored, err := session.Load(sessions[0].ID)
	if err != nil {
		t.Fatal(err)
	}
	if stored.Status != "failed" || stored.ExitCode == nil || *stored.ExitCode != 1 {
		t.Fatalf("reused-pid run stored as %s/%v, want failed/1", stored.Status, stored.ExitCode)
	}
}

// A match near the tail clamps the viewport to the bottom; follow must stay
// off anyway, or the next output scrolls the match away.
func TestSearchNearTailKeepsFollowOff(t *testing.T) {
	now := time.Now()
	s := &session.Session{ID: "tail0000-0000", CLI: "codex", Model: "gpt-5.5", Mode: "review", Status: "running",
		StartTime: now, PID: 101, WorkDir: "/src/p",
		LogFile: writeTempLog(t, "tail.log", strings.Repeat("filler\n", 200)+"NEEDLE\n")}
	m := openDetail(t, []*session.Session{s}, 120, 40)
	m = send(t, m, press("/"))
	m = send(t, m, typeText("needle")...)
	m = send(t, m, press("enter"))
	if len(m.detail.matches) != 1 {
		t.Fatalf("matches = %v", m.detail.matches)
	}
	if m.detail.follow {
		t.Fatal("search near the tail turned follow on")
	}
}

// Group refreshes can reorder members; the open member is anchored by ID.
func TestDetailMemberSurvivesMembershipChange(t *testing.T) {
	sessions := groupFixture(t)
	m := openDetail(t, sessions, 120, 40)
	m = send(t, m, press("]"))
	want := m.detail.current(m.list.selected()).ID
	var rest []*session.Session
	for _, s := range sessions {
		if s.ID != "gpt55000-0000" { // the first member in display order
			rest = append(rest, s)
		}
	}
	m = send(t, m, SessionEvent{Sessions: rest})
	if got := m.detail.current(m.list.selected()).ID; got != want {
		t.Fatalf("member switched from %s to %s after a refresh", want, got)
	}
}

// Leaving search with the help expanded must shrink the viewport back, or its
// last lines are clipped off the frame.
func TestDetailViewportResizesOnModeChange(t *testing.T) {
	m := openDetail(t, previewFixture(t), 120, 40)
	m = send(t, m, press("?"))
	expanded := m.detail.vp.Height()
	m = send(t, m, press("/"))
	m = send(t, m, SessionEvent{Sessions: previewFixture(t)})
	m = send(t, m, press("esc"))
	if got := m.detail.vp.Height(); got != expanded {
		t.Fatalf("viewport height %d after leaving search, want %d", got, expanded)
	}
	if lines := strings.Count(m.viewContent(), "\n") + 1; lines > 40 {
		t.Fatalf("frame has %d lines, terminal has 40", lines)
	}
}

// x on a "running" record whose process is gone offers nothing to stop.
func TestStopOnDeadRunningRecordSaysNothingRunning(t *testing.T) {
	m, rec := openKillModel(t, runningGroup(t)[:1])
	m.alive = func(int, int64) bool { return false }
	m = send(t, m, press("x"))
	if m.mode != modeDetail || m.detail.confirm != nil {
		t.Fatal("x on a dead running record opened the confirm bar")
	}
	if !strings.Contains(frameText(m), "nothing running") {
		t.Fatalf("no nothing-running notice:\n%s", frameText(m))
	}
	if len(rec.pids) != 0 {
		t.Fatalf("dead record signalled %v", rec.pids)
	}
}

// Codex finding 1: a live-looking record without a recorded PIDStart could be
// any process that reused the PID. x refuses it, even when alive says yes.
func TestStopRefusesARunWithoutRecordedStart(t *testing.T) {
	sessions := runningGroup(t)[:1]
	sessions[0].PIDStart = 0
	if err := sessions[0].Save(); err != nil {
		t.Fatal(err)
	}
	m, rec := openKillModel(t, sessions)
	m = send(t, m, press("x"))
	if m.mode != modeDetail || m.detail.confirm != nil {
		t.Fatal("x on an unverifiable run opened the confirm bar")
	}
	if !strings.Contains(frameText(m), "cannot verify process identity") {
		t.Fatalf("no unverified notice:\n%s", frameText(m))
	}
	if len(rec.pids) != 0 {
		t.Fatalf("unverifiable run signalled %v", rec.pids)
	}
}

// The confirm-time path refuses it too: no signal, even when alive says yes.
func TestStopSessionsNeverSignalsARunWithoutRecordedStart(t *testing.T) {
	sessions := runningGroup(t)[:1]
	sessions[0].PIDStart = 0
	if err := sessions[0].Save(); err != nil {
		t.Fatal(err)
	}
	m := newListModel(t, sessions, 120, 40)
	rec := &killRecorder{}
	m.kill = rec.kill
	m.alive = func(int, int64) bool { return true }
	m.stopSessions(sessions)
	if len(rec.pids) != 0 {
		t.Fatalf("unverifiable run signalled %v", rec.pids)
	}
	// Its process may still be alive, so it must not be rewritten either.
	stored, err := session.Load(sessions[0].ID)
	if err != nil {
		t.Fatal(err)
	}
	if stored.Status != "running" {
		t.Fatalf("unverifiable run rewritten as %s", stored.Status)
	}
}
