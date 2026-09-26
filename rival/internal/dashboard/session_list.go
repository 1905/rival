package dashboard

import (
	"fmt"
	"strings"
	"time"

	"github.com/1F47E/rival/internal/config"
	"github.com/1F47E/rival/internal/session"
	"github.com/1F47E/rival/internal/sessionview"
	"github.com/charmbracelet/x/ansi"
)

// fitWidth truncates a row to the terminal width by display cells. The column
// layout has a ~75-cell floor, so on a narrow terminal the assembled row would
// otherwise overflow and the terminal would hard-wrap it — which drags every
// following row out of place.
func fitWidth(s string, width int) string {
	if width <= 0 || ansi.StringWidth(s) <= width {
		return s
	}
	return ansi.Truncate(s, width, "")
}

func renderSessionList(items []displayItem, selected int, width, height int, hasMore bool, hiddenCount int) string {
	if len(items) == 0 {
		return fitWidth(labelStyle.Render("No sessions yet. Run rival to get started."), width)
	}

	var b strings.Builder

	// Header row.
	header := formatHeaderRow(width)
	b.WriteString(fitWidth(headerStyle.Render(header), width))
	b.WriteString("\n")

	maxItems := height - 2 // header + separator
	if hasMore {
		maxItems-- // reserve 1 line for "load more"
	}
	if maxItems < 1 {
		maxItems = 1
	}

	// Scroll offset.
	offset := 0
	if selected >= maxItems {
		offset = selected - maxItems + 1
	}

	for i := offset; i < len(items) && i-offset < maxItems; i++ {
		item := items[i]
		line := formatItemRow(&item, width)
		if i == selected {
			b.WriteString(fitWidth(selectedItemStyle.Render(line), width))
		} else {
			b.WriteString(fitWidth(normalItemStyle.Render(line), width))
		}
		b.WriteString("\n")
	}

	if hasMore {
		more := fmt.Sprintf("  ▼ %d more — press l to load", hiddenCount)
		b.WriteString(fitWidth(labelStyle.Render(more), width))
		b.WriteString("\n")
	}

	return b.String()
}

func formatHeaderRow(width int) string {
	cols := calcColumns(width)
	return fmt.Sprintf(" %-*s %-*s %-*s %-*s %-*s %-*s %s",
		cols.status, "STATUS",
		cols.cli, "REVIEWER",
		cols.model, "MODEL",
		cols.effort, "EFFORT",
		cols.elapsed, "TIME",
		cols.workdir, "WORKDIR",
		"PROMPT",
	)
}

func formatItemRow(item *displayItem, width int) string {
	if item.IsGroup() {
		return formatGroupRow(item, width)
	}
	return formatSessionRow(item.Primary(), width)
}

// CLI icons — Unicode symbols for visual distinction.
const (
	iconSol      = "◈" // Sol
	iconClaude   = "⬡" // Claude
	iconOpencode = "❯" // OpenCode model
	iconGrok     = "𝕏" // Grok
	iconPlan     = "▤" // Plan/spec review
	iconAntislop = "⌁" // Antislop quality review
	iconSecurity = "⚿" // Security review
)

// cliLabel returns a display label with icon for a CLI name.
func cliLabel(cli, model, mode string) string {
	// A solo security session must show its own identity; without this it
	// falls through to its CLI arm and looks like an ordinary opencode run.
	if mode == session.ModeSecurity {
		return iconSecurity + " sec"
	}
	if mode == "plan" {
		return iconPlan + " plan"
	}
	switch cli {
	case "codex":
		return iconSol + " " + config.EngineLabel(cli, model)
	case "claude", "fable":
		// "fable" is read-compat for sessions written by releases before 3.34.
		if mode == "docker" {
			return iconClaude + " " + config.EngineLabel(cli, model) + "/dk"
		}
		return iconClaude + " " + config.EngineLabel(cli, model)
	case "opencode", "kimi":
		// "kimi" is read-compat for sessions written by the short-lived
		// kimi-cli transport (removed; K3 now runs through opencode).
		return iconOpencode + " " + config.EngineLabel(cli, model)
	case "grok":
		return iconGrok + " " + config.EngineLabel(cli, model)
	default:
		return cli
	}
}

func formatGroupRow(item *displayItem, width int) string {
	cols := calcColumns(width)
	s := item.Primary()

	// Status: worst of the group (running > failed > completed).
	status := groupStatus(item)
	icon := statusIcon(status)
	statusText := fmt.Sprintf("%s %s", icon, status)

	// Elapsed: max of the group.
	elapsed := groupElapsed(item)

	wd := truncate(s.WorkDir, cols.workdir)
	prompt := ""
	if cols.prompt > 0 {
		prompt = truncate(s.PromptPreview, cols.prompt)
	}

	rawStatus := fmt.Sprintf("%-*s", cols.status, statusText)
	coloredStatus := statusStyle(status).Render(rawStatus)

	return fmt.Sprintf(" %s %-*s %-*s %-*s %-*s %-*s %s",
		coloredStatus,
		cols.cli, groupIcon(item),
		cols.model, truncate(groupModels(item), cols.model),
		cols.effort, groupEffort(item),
		cols.elapsed, elapsed,
		cols.workdir, wd,
		prompt,
	)
}

// The derivations below delegate to internal/sessionview so the TUI and the
// web dashboard cannot disagree. Only presentation stays here.

func groupEffort(item *displayItem) string {
	return sessionview.Effort(item.Sessions)
}

// groupIcon returns the list-row icon and label for a group.
func groupIcon(item *displayItem) string {
	switch sessionview.Kind(item.Sessions) {
	case "security":
		return iconSecurity + " sec"
	case "antislop":
		return iconAntislop + " slop"
	case "plan":
		return iconPlan + " plan"
	}
	reviewers := 0
	for _, s := range item.Sessions {
		if s.Mode == "megareview" {
			reviewers++
		}
	}
	if reviewers == 0 {
		reviewers = 1
	}
	return strings.Repeat(iconOpencode, reviewers) + " mega"
}

// groupKindLabel returns the human title word for a group.
func groupKindLabel(item *displayItem) string {
	return sessionview.Kind(item.Sessions)
}

// groupCLIs returns the group's distinct public model names joined with "+".
func groupCLIs(item *displayItem) string {
	return sessionview.JoinLabels(sessionview.EngineLabels(item.Sessions), "+")
}

// groupModels returns the distinct public model names in display order.
func groupModels(item *displayItem) string {
	return sessionview.JoinLabels(sessionview.EngineLabels(item.Sessions), " + ")
}

func groupStatus(item *displayItem) string {
	return sessionview.Status(item.Sessions)
}

// groupElapsed is the wall-clock span of the whole group. It previously
// reported the longest single member, which disagreed with the web dashboard.
func groupElapsed(item *displayItem) string {
	return sessionview.Elapsed(item.Sessions)
}

func formatSessionRow(s *session.Session, width int) string {
	cols := calcColumns(width)

	// Status icon + text. Queued rows show the position in line.
	icon := statusIcon(s.Status)
	statusText := fmt.Sprintf("%s %s", icon, s.Status)
	if s.Status == "queued" && s.QueuePosition > 0 {
		statusText = fmt.Sprintf("%s queued #%d", icon, s.QueuePosition)
	}

	// Elapsed time.
	elapsed := formatElapsed(s)

	// Truncate workdir and prompt to fit.
	wd := truncate(s.WorkDir, cols.workdir)
	prompt := ""
	if cols.prompt > 0 {
		prompt = truncate(s.PromptPreview, cols.prompt)
	}

	// Build raw line without ANSI for proper alignment, then apply status color.
	rawStatus := fmt.Sprintf("%-*s", cols.status, statusText)
	coloredStatus := statusStyle(s.Status).Render(rawStatus)

	return fmt.Sprintf(" %s %-*s %-*s %-*s %-*s %-*s %s",
		coloredStatus,
		cols.cli, cliLabel(s.CLI, s.Model, s.Mode),
		cols.model, truncate(config.EngineLabel(s.CLI, s.Model), cols.model),
		cols.effort, s.Effort,
		cols.elapsed, elapsed,
		cols.workdir, wd,
		prompt,
	)
}

type columnWidths struct {
	status  int
	cli     int
	model   int
	effort  int
	elapsed int
	workdir int
	prompt  int
}

func calcColumns(width int) columnWidths {
	// Fixed columns.
	c := columnWidths{
		status:  12,
		cli:     10,
		model:   28,
		effort:  8,
		elapsed: 8,
	}

	// 2 for leading space + separators between columns (7 spaces for 8 columns).
	fixed := 2 + c.status + c.cli + c.model + c.effort + c.elapsed + 7
	remaining := width - fixed
	if remaining < 10 {
		remaining = 10
	}

	// Split remaining between workdir and prompt.
	c.workdir = remaining / 2
	c.prompt = remaining - c.workdir

	return c
}

func statusIcon(status string) string {
	switch status {
	case "running":
		return "●"
	case "completed":
		return "●"
	case "failed":
		return "●"
	case "queued":
		return "◌"
	default:
		return "○"
	}
}

func formatElapsed(s *session.Session) string {
	if s.Duration != "" {
		return s.Duration
	}
	if s.Status == "running" {
		d := time.Since(s.StartTime).Round(time.Second)
		return d.String()
	}
	// Queued: show how long it has been waiting in line.
	if s.Status == "queued" && s.QueuedAt != nil {
		return time.Since(*s.QueuedAt).Round(time.Second).String()
	}
	return "-"
}

func truncate(s string, max int) string {
	if max <= 0 {
		return ""
	}
	runes := []rune(s)
	if len(runes) <= max {
		return s
	}
	if max <= 3 {
		return string(runes[:max])
	}
	return string(runes[:max-1]) + "…"
}
