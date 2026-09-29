package dashboard

import (
	"fmt"
	"strings"
	"time"

	"github.com/1905/rival/internal/session"
	"github.com/charmbracelet/x/ansi"
)

// previewTailLines caps how many log lines the preview keeps. It is far more
// than any pane shows, and bounds the wrap work on a 1s tick.
const previewTailLines = 200

// previewPane is the right-hand pane: the selected run's meta block plus the
// tail of its log. body is rendered by refresh, so View never touches a file.
type previewPane struct {
	body string
	// tail caches the log tail by file state: the meta block is cheap and
	// re-rendered every refresh, the log only when it changed.
	tail logCache
}

// refresh re-renders the preview for item at width×height. The log is re-read
// only when the tailed file, the width or the row count changed.
func (p *previewPane) refresh(item *displayItem, width, height int) {
	if item == nil || item.Primary() == nil || width <= 0 || height <= 0 {
		p.body = ""
		return
	}
	p.body = renderPreview(item, width, height, &p.tail)
}

// view returns exactly height lines of exactly width cells.
func (p previewPane) view(width, height int) string {
	if width <= 0 || height <= 0 {
		return ""
	}
	var lines []string
	if p.body != "" {
		lines = strings.Split(p.body, "\n")
	}
	if len(lines) > height {
		lines = lines[:height]
	}
	out := make([]string, height)
	for i := range out {
		l := ""
		if i < len(lines) {
			l = lines[i]
		}
		out[i] = fitCell(l, width)
	}
	return strings.Join(out, "\n")
}

// itemLive reports whether the run can still change: running, or queued with a
// growing wait time.
func itemLive(item *displayItem) bool {
	return isLive(itemStatus(item))
}

// isLive reports whether a status means "not finished yet": running, or
// queued and waiting for a slot.
func isLive(status string) bool {
	return status == "running" || status == "queued"
}

// renderPreview builds the meta block and fills the rest with the log tail.
func renderPreview(item *displayItem, width, height int, cache *logCache) string {
	lines := previewMeta(item, width)
	lines = append(lines, dimStyle.Render(ansi.Truncate("─ output (tail) ─", width, "")))
	if len(lines) >= height {
		return strings.Join(lines[:height], "\n")
	}
	lines = append(lines, previewTail(tailSession(item), width, height-len(lines), cache)...)
	return strings.Join(lines, "\n")
}

// previewMeta is the block above the log: who, what, when, and for a group
// one line per member. Every line is cut to width.
func previewMeta(item *displayItem, width int) []string {
	s := item.Primary()
	cut := func(l string) string { return ansi.Truncate(l, width, "…") }
	var lines []string
	if item.IsGroup() {
		lines = append(lines,
			cut(joinMeta(textStyle.Render(projectName(s.WorkDir)), textStyle.Render(kindLabel(item)), dimStyle.Render(fmt.Sprintf("%d models", len(item.Sessions))))),
			cut(joinMeta(dimStyle.Render(groupEffort(item)), textStyle.Render(groupElapsed(item)))),
			cut(dimStyle.Render(startedLine(s.StartTime, 0))),
		)
		for _, m := range item.Sessions {
			status := m.Status
			line := statusStyle(status).Render(statusGlyph(status, "●")) + " " + valueStyle.Render(modelName(m)) + "  " + statusStyle(status).Render(status)
			if m.Mode == "consilium" {
				line += dimStyle.Render(" · judge")
			}
			lines = append(lines, cut(line))
		}
	} else {
		lines = append(lines,
			cut(joinMeta(textStyle.Render(projectName(s.WorkDir)), textStyle.Render(kindLabel(item)), dimStyle.Render(s.CLI))),
			cut(joinMeta(valueStyle.Render(modelName(s)), dimStyle.Render(s.Effort), statusStyle(s.Status).Render(formatElapsed(s)))),
			cut(dimStyle.Render(startedLine(s.StartTime, s.PID))),
		)
	}
	if scope := oneLine(s.ReviewScope); scope != "" {
		lines = append(lines, cut(dimStyle.Render("─ scope ─")))
		wrapped := strings.Split(ansi.Hardwrap(scope, max(1, width), true), "\n")
		if len(wrapped) > 2 {
			wrapped = append(wrapped[:1], wrapped[1]+"…")
		}
		for _, w := range wrapped {
			lines = append(lines, cut(textStyle.Render(w)))
		}
	}
	return lines
}

// joinMeta joins non-empty rendered parts with a dim middle dot.
func joinMeta(parts ...string) string {
	kept := parts[:0]
	for _, p := range parts {
		if ansi.Strip(p) != "" {
			kept = append(kept, p)
		}
	}
	return strings.Join(kept, dimStyle.Render(" · "))
}

// startedLine reads "started 11:40 · pid 81233"; a run from another day gets
// its date too.
func startedLine(t time.Time, pid int) string {
	line := "started -"
	if !t.IsZero() {
		t = t.Local()
		layout := "15:04"
		if sectionFor(t, time.Now()) != "TODAY" {
			layout = "Jan 02 15:04"
		}
		line = "started " + t.Format(layout)
	}
	if pid > 0 {
		line += fmt.Sprintf(" · pid %d", pid)
	}
	return line
}

// tailSession is the member whose log the preview tails: the judge when the
// group has one (its verdict is the result), else the last member.
func tailSession(item *displayItem) *session.Session {
	for _, s := range item.Sessions {
		if s.Mode == "consilium" {
			return s
		}
	}
	return item.Sessions[len(item.Sessions)-1]
}

// previewTail is the last rows wrapped lines of s's log, or a dim note when
// there is nothing to show.
func previewTail(s *session.Session, width, rows int, cache *logCache) []string {
	lines, err := cache.read(s, width, min(rows, previewTailLines))
	switch {
	case err != nil:
		return []string{dimStyle.Render(ansi.Truncate(fmt.Sprintf("(log unavailable: %v)", err), width, "…"))}
	case len(lines) == 0:
		return []string{dimStyle.Render("(empty log)")}
	}
	if len(lines) > rows {
		lines = lines[len(lines)-rows:]
	}
	return lines
}
