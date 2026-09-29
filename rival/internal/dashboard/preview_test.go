package dashboard

import (
	"os"
	"strings"
	"testing"
	"time"

	"github.com/1905/rival/internal/session"
	"github.com/charmbracelet/x/ansi"
)

func previewLines(t *testing.T, p previewPane, width, height int) []string {
	t.Helper()
	lines := strings.Split(p.view(width, height), "\n")
	if len(lines) != height {
		t.Fatalf("preview has %d lines, want %d", len(lines), height)
	}
	for i, l := range lines {
		if w := ansi.StringWidth(l); w > width {
			t.Fatalf("line %d is %d cells, wider than %d: %q", i, w, width, l)
		}
	}
	return lines
}

func TestPreviewSingleRun(t *testing.T) {
	var log strings.Builder
	for i := range 100 {
		log.WriteString("log line ")
		log.WriteString(strings.Repeat("x", i%7))
		log.WriteString("\n")
	}
	log.WriteString("VERDICT: 6/10\n")
	item := &displayItem{Sessions: []*session.Session{{
		ID: "a1b2c3d4-single", CLI: "codex", Model: "gpt-6-astra", Mode: "review", Effort: "xhigh",
		Status: "completed", Duration: "1m31s", StartTime: time.Date(2026, 9, 26, 11, 40, 0, 0, time.Local), PID: 81233,
		WorkDir: "/src/orbit-web", ReviewScope: "plans/2026-09-26-service-identity",
		LogFile: writeTempLog(t, "single.log", log.String()),
	}}}

	var p previewPane
	p.refresh(item, 60, 20)
	text := ansi.Strip(strings.Join(previewLines(t, p, 60, 20), "\n"))
	for _, want := range []string{
		"orbit-web", "review", "codex", "gpt-6-astra", "xhigh", "1m31s",
		"started", "11:40", "pid 81233", "plans/2026-09-26-service-identity",
		"output (tail)", "VERDICT: 6/10",
	} {
		if !strings.Contains(text, want) {
			t.Errorf("preview lacks %q:\n%s", want, text)
		}
	}
	if strings.Index(text, "VERDICT") < strings.Index(text, "output (tail)") {
		t.Fatal("the tail comes before the output heading")
	}
	// The pane shows the END of the log: the last line sits on the last row.
	lines := strings.Split(text, "\n")
	if !strings.Contains(lines[len(lines)-1], "VERDICT: 6/10") {
		t.Fatalf("last row = %q, want the last log line", lines[len(lines)-1])
	}
}

func TestPreviewGroupListsEveryMember(t *testing.T) {
	gid := "g-1"
	now := time.Now()
	item := &displayItem{Sessions: []*session.Session{
		{ID: "m1", GroupID: gid, CLI: "codex", Model: "gpt-5.5", Mode: "review", Status: "completed", StartTime: now, WorkDir: "/src/mathquest",
			LogFile: writeTempLog(t, "m1.log", "reviewer one output\n")},
		{ID: "m2", GroupID: gid, CLI: "opencode", Model: "gemini-3.1", Mode: "review", Status: "failed", StartTime: now, WorkDir: "/src/mathquest",
			LogFile: writeTempLog(t, "m2.log", "reviewer two output\n")},
		{ID: "m3", GroupID: gid, CLI: "claude", Model: "claude-opus-5-5", Mode: "consilium", Status: "completed", StartTime: now, WorkDir: "/src/mathquest",
			LogFile: writeTempLog(t, "m3.log", "JUDGE VERDICT\n")},
	}}

	var p previewPane
	p.refresh(item, 70, 24)
	lines := previewLines(t, p, 70, 24)
	for _, want := range []struct{ glyph, model, status string }{
		{"✓", "gpt-5.5", "completed"},
		{"✗", "gemini-3.1", "failed"},
		{"✓", "claude-opus-5-5", "completed"},
	} {
		found := false
		for _, l := range lines {
			l = ansi.Strip(l)
			if strings.Contains(l, want.glyph) && strings.Contains(l, want.model) && strings.Contains(l, want.status) {
				found = true
				break
			}
		}
		if !found {
			t.Errorf("no member line with %s %s %s:\n%s", want.glyph, want.model, want.status, ansi.Strip(strings.Join(lines, "\n")))
		}
	}
	text := ansi.Strip(strings.Join(lines, "\n"))
	if !strings.Contains(text, "JUDGE VERDICT") {
		t.Errorf("group preview does not tail the judge log:\n%s", text)
	}
	if strings.Contains(text, "reviewer one output") {
		t.Errorf("group preview tails a reviewer instead of the judge:\n%s", text)
	}
}

func TestPreviewGroupWithoutJudgeTailsLastMember(t *testing.T) {
	gid := "g-2"
	item := &displayItem{Sessions: []*session.Session{
		{ID: "p1", GroupID: gid, CLI: "codex", Model: "gpt-5.5", Mode: "plan", Status: "completed",
			LogFile: writeTempLog(t, "p1.log", "first plan\n")},
		{ID: "p2", GroupID: gid, CLI: "claude", Model: "claude-opus-5-5", Mode: "plan", Status: "completed",
			LogFile: writeTempLog(t, "p2.log", "second plan\n")},
	}}
	var p previewPane
	p.refresh(item, 60, 20)
	text := ansi.Strip(p.view(60, 20))
	if !strings.Contains(text, "second plan") || strings.Contains(text, "first plan") {
		t.Fatalf("want the last member's tail only:\n%s", text)
	}
}

func TestPreviewMissingLog(t *testing.T) {
	item := &displayItem{Sessions: []*session.Session{{
		ID: "missing", CLI: "codex", Model: "gpt-5.5", Status: "completed",
		LogFile: t.TempDir() + "/nope.log",
	}}}
	var p previewPane
	p.refresh(item, 60, 16)
	text := ansi.Strip(strings.Join(previewLines(t, p, 60, 16), "\n"))
	if !strings.Contains(text, "(log unavailable:") {
		t.Fatalf("missing log not reported:\n%s", text)
	}
}

func TestPreviewLinesFitEveryWidth(t *testing.T) {
	item := &displayItem{Sessions: []*session.Session{{
		ID: "wide", CLI: "codex", Model: "claude-opus-4-6[1m]", Mode: "review", Effort: "ultra", Status: "running",
		StartTime: time.Now(), PID: 1, WorkDir: "/src/日本語のプロジェクト名前はとても長い",
		ReviewScope: strings.Repeat("scope/path/that/is/long ", 20),
		LogFile:     writeTempLog(t, "wide.log", nastyLog+strings.Repeat("padding\n", 50)),
	}}}
	for _, w := range []int{10, 30, 53, 89} {
		for _, h := range []int{1, 5, 30} {
			var p previewPane
			p.refresh(item, w, h)
			previewLines(t, p, w, h)
		}
	}
}

func TestPreviewRefreshSkipsRereadForFinishedRun(t *testing.T) {
	reads := 0
	orig := readTail
	readTail = func(path string, maxBytes int64) ([]byte, bool, error) {
		reads++
		return orig(path, maxBytes)
	}
	t.Cleanup(func() { readTail = orig })

	s := &session.Session{ID: "done", CLI: "codex", Model: "gpt-5.5", Status: "completed",
		LogFile: writeTempLog(t, "done.log", "finished\n")}
	item := &displayItem{Sessions: []*session.Session{s}}

	var p previewPane
	p.refresh(item, 60, 20)
	p.refresh(item, 60, 20)
	if reads != 1 {
		t.Fatalf("finished run read %d times, want 1", reads)
	}

	// A resize has to re-wrap, so it re-reads.
	p.refresh(item, 70, 20)
	if reads != 2 {
		t.Fatalf("resize read count = %d, want 2", reads)
	}

	// A running run re-reads once its log grows, and not while it is idle.
	s.Status = "running"
	p.refresh(item, 70, 20)
	if reads != 2 {
		t.Fatalf("idle running run read count = %d, want 2", reads)
	}
	appendToLog(t, s.LogFile, "more output")
	p.refresh(item, 70, 20)
	if reads != 3 {
		t.Fatalf("grown running run read count = %d, want 3", reads)
	}
	if !strings.Contains(ansi.Strip(p.view(70, 20)), "more output") {
		t.Fatal("preview does not show the appended line")
	}
}

func appendToLog(t *testing.T, path, line string) {
	t.Helper()
	f, err := os.OpenFile(path, os.O_APPEND|os.O_WRONLY, 0o600)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = f.Close() }()
	if _, err := f.WriteString(line + "\n"); err != nil {
		t.Fatal(err)
	}
}
