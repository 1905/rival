package dashboard

import (
	"os"
	"slices"
	"strings"

	"github.com/1905/rival/internal/logfmt"
	"github.com/1905/rival/internal/session"
	"github.com/charmbracelet/x/ansi"
)

// sanitizeLog strips terminal control sequences and then expands tabs. A tab
// is one rune but many cells, so leaving it in place makes wrapped lines
// overflow the terminal.
func sanitizeLog(raw string) string {
	return logfmt.ExpandTabs(logfmt.Sanitize(raw), logfmt.TabWidth)
}

// readTail is logfmt.ReadTail behind a seam, so tests can count file reads.
var readTail = logfmt.ReadTail

// previewTailBytes caps the preview's read. It keeps previewTailLines lines of
// any sane log, and the preview never shows more.
const previewTailBytes int64 = 32 << 10

// rawCutSlack is how many extra raw lines a lastN cut keeps before
// sanitizing. Trailing lines that sanitize to nothing (a bare colour reset)
// are trimmed afterwards, and the slack keeps them from costing tail rows.
const rawCutSlack = 16

// readLogLines reads one session log, strips terminal control sequences, and
// hard-wraps by display width so wide runes and tabs cannot push a line past
// wrapWidth. A missing log is an error, an empty one nil. lastN > 0 is the
// preview: it reads at most previewTailBytes and keeps only the last lastN
// source lines, cut before sanitizing and wrapping, so a small pane does not
// process 256 KB it will never show; the "earlier output omitted" marker is
// then left out, since the caller cut the text itself.
func readLogLines(s *session.Session, wrapWidth, lastN int) ([]string, error) {
	// Tail-only: the detail view rebuilds this on every 1s tick, and wrapping a
	// whole multi-megabyte log takes about as long as the tick interval.
	maxBytes := logfmt.MaxTailBytes
	if lastN > 0 {
		maxBytes = min(maxBytes, previewTailBytes)
	}
	data, truncated, err := readTail(s.LogFile, maxBytes)
	if err != nil {
		return nil, err
	}
	if len(data) == 0 {
		return nil, nil
	}

	raw := string(data)
	if lastN > 0 {
		// Sanitizing works line by line, so cutting raw lines first is safe.
		raw = strings.TrimRight(raw, "\n")
		if i := nthLastNewline(raw, lastN+rawCutSlack); i >= 0 {
			raw = raw[i+1:]
		}
	}
	// No public model naming: the TUI shows the raw model id everywhere, so
	// the log names the same model the list does.
	text := strings.TrimRight(sanitizeLog(raw), "\n")
	if lastN > 0 {
		if i := nthLastNewline(text, lastN); i >= 0 {
			text = text[i+1:]
		}
	} else if truncated {
		text = labelStyle.Render("... earlier output omitted — press o to open the full log") + "\n" + text
	}
	if wrapWidth > 0 {
		text = ansi.Hardwrap(text, wrapWidth, true)
	}
	return strings.Split(text, "\n"), nil
}

// logCache holds one log's wrapped lines plus the file state and wrap they
// were built from. A log whose size and mtime are unchanged is neither
// re-read nor re-wrapped, so a 1s tick on an idle log costs one stat.
type logCache struct {
	key   logKey
	lines []string
	ok    bool
}

type logKey struct {
	path         string
	size, mtime  int64
	width, lastN int
}

// read returns what readLogLines(s, width, lastN) would, re-reading only when
// the file or the wrap changed. The result is clipped, so a caller appending
// to it never writes into the cache.
func (c *logCache) read(s *session.Session, width, lastN int) ([]string, error) {
	fi, err := os.Stat(s.LogFile)
	if err != nil {
		// Let the read report the error, so the message stays the same.
		c.ok = false
		return readLogLines(s, width, lastN)
	}
	key := logKey{s.LogFile, fi.Size(), fi.ModTime().UnixNano(), width, lastN}
	if c.ok && c.key == key {
		return slices.Clip(c.lines), nil
	}
	lines, err := readLogLines(s, width, lastN)
	c.key, c.lines, c.ok = key, lines, err == nil
	return slices.Clip(lines), err
}

// nthLastNewline is the index of the n-th newline from the end of s, or -1
// when s has fewer than n newlines.
func nthLastNewline(s string, n int) int {
	i := len(s)
	for ; n > 0; n-- {
		i = strings.LastIndexByte(s[:i], '\n')
		if i < 0 {
			return -1
		}
	}
	return i
}
