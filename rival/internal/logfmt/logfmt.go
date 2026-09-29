// Package logfmt makes raw CLI logs safe to display. Runtime logs are captured
// verbatim, so they carry ANSI escapes, carriage-return progress frames and
// stray control bytes that neither a terminal pane nor a browser renders
// sensibly. The TUI (internal/dashboard) uses Sanitize and then expands tabs,
// because a tab is one rune but many terminal cells.
package logfmt

import (
	"bytes"
	"io"
	"os"
	"strings"

	"github.com/charmbracelet/x/ansi"
)

// TabWidth is how many spaces ExpandTabs substitutes for one tab.
const TabWidth = 4

// SanitizeLine makes one raw log line safe to display: it keeps only the last
// progress frame, strips ANSI escape sequences, and drops C0 control chars and
// DEL. Tabs survive — expand them separately with ExpandTabs when the renderer
// has no tab semantics of its own.
//
// The step order is load-bearing: Sanitize splits on "\n", so every line of a
// CRLF log arrives with a trailing "\r". Trimming that single terminator first
// is what keeps the progress-frame rule (keep only what follows the last
// remaining "\r") from blanking every line of such a log.
func SanitizeLine(line string) string {
	line = strings.TrimSuffix(line, "\r")
	if idx := strings.LastIndex(line, "\r"); idx >= 0 {
		line = line[idx+1:]
	}
	line = ansi.Strip(line)

	var b strings.Builder
	b.Grow(len(line))
	for _, r := range line {
		if r == '\t' {
			b.WriteRune(r)
			continue
		}
		if r < 0x20 || r == 0x7f {
			continue
		}
		b.WriteRune(r)
	}
	return b.String()
}

// Sanitize applies SanitizeLine to every line of raw, preserving newlines.
func Sanitize(raw string) string {
	lines := strings.Split(raw, "\n")
	for i, line := range lines {
		lines[i] = SanitizeLine(line)
	}
	return strings.Join(lines, "\n")
}

// ExpandTabs replaces every tab with width spaces. Terminal panes need this
// before wrapping: a tab is one rune but many cells, so leaving it in place
// makes wrapped lines overflow. A non-positive width removes tabs entirely,
// matching strings.Repeat semantics.
func ExpandTabs(s string, width int) string {
	if width < 0 {
		width = 0
	}
	return strings.ReplaceAll(s, "\t", strings.Repeat(" ", width))
}

// MaxTailBytes is how much of a log's tail a display surface reads. Logs reach
// tens of megabytes (62 MB observed), and a renderer that re-reads on a timer
// cannot afford the whole file: wrapping 62 MB measured ~970ms, against a 1s
// refresh tick. Earlier output stays reachable by opening the log itself.
const MaxTailBytes int64 = 256 << 10

// ReadTail returns up to MaxTailBytes from the end of path, reporting whether
// the file was truncated.
//
// A tail that starts mid-file is aligned past the first newline: the raw offset
// otherwise decapitates whatever multi-byte rune or ANSI escape straddles it,
// and a beheaded escape renders as literal garbage. Escapes do not span lines,
// so one alignment fixes both. A single enormous line has no newline to align
// to — serving the unaligned tail beats serving nothing.
func ReadTail(path string, maxBytes int64) ([]byte, bool, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, false, err
	}
	defer func() { _ = f.Close() }()

	info, err := f.Stat()
	if err != nil {
		return nil, false, err
	}
	truncated := info.Size() > maxBytes
	if truncated {
		if _, err := f.Seek(-maxBytes, io.SeekEnd); err != nil {
			return nil, false, err
		}
	}

	data, err := io.ReadAll(io.LimitReader(f, maxBytes))
	if err != nil {
		return nil, false, err
	}
	if truncated {
		if idx := bytes.IndexByte(data, '\n'); idx >= 0 && idx+1 < len(data) {
			data = data[idx+1:]
		}
	}
	return []byte(strings.ToValidUTF8(string(data), "")), truncated, nil
}
