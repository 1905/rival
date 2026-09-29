package dashboard

import (
	"fmt"
	"os"
	"strings"
	"testing"

	"github.com/1905/rival/internal/session"
	"github.com/charmbracelet/x/ansi"
)

func TestSanitizeLog(t *testing.T) {
	tests := []struct {
		name string
		raw  string
		want string
	}{
		{"tab expands to four spaces", "if x {\n\treturn\n}", "if x {\n    return\n}"},
		{"progress frames collapse to the last", "10%\r99%", "99%"},
		{"csi color stripped", "\x1b[31mred\x1b[0m", "red"},
		{"osc bel stripped", "\x1b]0;title\x07text", "text"},
		{"osc st stripped", "\x1b]8;;url\x1b\\link", "link"},
		{"backspace and nul dropped", "a\x08b\x00c", "abc"},
		{"newlines preserved", "one\ntwo\nthree", "one\ntwo\nthree"},
		{"plain text unchanged", "plain log line", "plain log line"},
		// CRLF: sanitizeLog splits on "\n", so each line still ends in "\r".
		// The trailing-terminator trim must run before the progress-frame rule
		// or every line of a CRLF log collapses to empty.
		{"crlf lines survive", "alpha\r\nbeta\r\n", "alpha\nbeta\n"},
		{"trailing frame keeps its text", "working...\r", "working..."},
		{"crlf plus progress frames", "10%\r99%\r\n", "99%\n"},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			if got := sanitizeLog(tc.raw); got != tc.want {
				t.Fatalf("sanitizeLog(%q) = %q, want %q", tc.raw, got, tc.want)
			}
		})
	}
}

// The original bug: wrapping by rune count let tabs and wide runes push lines
// past the pane width, so the terminal hard-wrapped them itself and bubbletea's
// repaint desynced. Every emitted line must fit in wrapWidth display cells.
func TestWrapLogLinesDisplayWidth(t *testing.T) {
	logPath := t.TempDir() + "/session.log"
	raw := strings.Join([]string{
		"func main() {",
		"\tif err := run(); err != nil {",
		"\t\tlog.Fatal(err)",
		"\t}",
		"}",
		strings.Repeat("あ", 50),
		"\x1b[31m" + strings.Repeat("error detail ", 20) + "\x1b[0m",
	}, "\n")
	if err := os.WriteFile(logPath, []byte(raw), 0o600); err != nil {
		t.Fatal(err)
	}
	s := &session.Session{CLI: "codex", Model: "gpt-5", LogFile: logPath}

	for _, width := range []int{20, 40, 80} {
		t.Run(fmt.Sprintf("width %d", width), func(t *testing.T) {
			lines, err := readLogLines(s, width, 0)
			if err != nil || len(lines) == 0 {
				t.Fatalf("readLogLines(width=%d) returned no lines (err %v)", width, err)
			}
			for i, line := range lines {
				if got := ansi.StringWidth(line); got > width {
					t.Fatalf("line %d width = %d, want <= %d: %q", i, got, width, line)
				}
			}
		})
	}
}

func TestWrapLogLinesEmptyAndMissing(t *testing.T) {
	empty := t.TempDir() + "/empty.log"
	if err := os.WriteFile(empty, nil, 0o600); err != nil {
		t.Fatal(err)
	}
	if got, err := readLogLines(&session.Session{LogFile: empty}, 80, 0); got != nil || err != nil {
		t.Fatalf("empty log = %q, %v, want nil, nil", got, err)
	}
	if got, err := readLogLines(&session.Session{LogFile: t.TempDir() + "/missing.log"}, 80, 0); got != nil || err == nil {
		t.Fatalf("missing log = %q, %v, want nil and an error", got, err)
	}
}

// The TUI shows raw model ids, so the log must too: no public renaming.
func TestWrapLogLinesKeepsRawModelID(t *testing.T) {
	logPath := t.TempDir() + "/raw.log"
	raw := "OpenAI Codex v1\nmodel: gpt-6-astra\n\x1b[31mred\x1b[0m\tend\n"
	if err := os.WriteFile(logPath, []byte(raw), 0o600); err != nil {
		t.Fatal(err)
	}
	lines, _ := readLogLines(&session.Session{CLI: "codex", Model: "gpt-6-astra", LogFile: logPath}, 80, 0)
	got := strings.Join(lines, "\n")
	if !strings.Contains(got, "model: gpt-6-astra") || !strings.Contains(got, "OpenAI Codex v1") {
		t.Fatalf("log was renamed: %q", got)
	}
	if strings.Contains(got, "\x1b") || strings.Contains(got, "\t") {
		t.Fatalf("ANSI or tab survived: %q", got)
	}
	if !strings.Contains(got, "red    end") {
		t.Fatalf("tab not expanded: %q", got)
	}
}

// The preview cuts raw lines before sanitizing. That must give the same tail
// as sanitizing the whole log first, even when the last lines sanitize to
// nothing or end in CRLF.
func TestReadLogLinesLastNMatchesTheFullRead(t *testing.T) {
	var b strings.Builder
	for i := range 60 {
		fmt.Fprintf(&b, "\tline %d 10%%\r99%%\r\n", i)
	}
	b.WriteString("\x1b[0m\n\x1b[0m\r\n\n")
	s := &session.Session{LogFile: writeTempLog(t, "cut.log", b.String())}
	full, err := readLogLines(s, 0, 0)
	if err != nil {
		t.Fatal(err)
	}
	for _, n := range []int{1, 5, 30, 200} {
		got, err := readLogLines(s, 0, n)
		if err != nil {
			t.Fatal(err)
		}
		want := full[max(0, len(full)-n):]
		if strings.Join(got, "\n") != strings.Join(want, "\n") {
			t.Fatalf("lastN %d = %q, want %q", n, got, want)
		}
	}
}

func TestCreateLogViewWritesTheRawLog(t *testing.T) {
	logPath := t.TempDir() + "/raw.log"
	raw := "OpenAI Codex v1\n--------\nmodel: gpt-6-astra\n"
	if err := os.WriteFile(logPath, []byte(raw), 0o600); err != nil {
		t.Fatal(err)
	}
	viewPath, err := createLogView(&session.Session{CLI: "codex", Model: "gpt-6-astra", LogFile: logPath})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Remove(viewPath) })
	data, err := os.ReadFile(viewPath)
	if err != nil {
		t.Fatal(err)
	}
	if string(data) != raw {
		t.Fatalf("log view = %q, want the raw log %q", data, raw)
	}
}

func TestCreateGroupLogViewUsesRawIDsAndErrors(t *testing.T) {
	dir := t.TempDir()
	review := dir + "/review.log"
	judge := dir + "/judge.log"
	_ = os.WriteFile(review, []byte("review body gpt-5.5\n"), 0o600)
	_ = os.WriteFile(judge, []byte("judge body\n"), 0o600)
	viewPath, err := createGroupLogView([]*session.Session{
		{CLI: "codex", Model: "gpt-5.5", Mode: "megareview", Effort: "high", Status: "failed", ErrorMsg: "codex gpt-5.5 exploded", LogFile: review},
		{CLI: "codex", Model: "gpt-6-astra", Mode: "consilium", Status: "completed", LogFile: judge},
	})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Remove(viewPath) })
	data, _ := os.ReadFile(viewPath)
	got := string(data)
	for _, want := range []string{"=== gpt-5.5 REVIEW · EFFORT high (FAILED) ===", "Error: codex gpt-5.5 exploded", "review body gpt-5.5", "=== gpt-6-astra JUDGE ===", "judge body"} {
		if !strings.Contains(got, want) {
			t.Fatalf("group log lacks %q:\n%s", want, got)
		}
	}
}
