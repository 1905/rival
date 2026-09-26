package dashboard

import (
	"fmt"
	"strings"

	"charm.land/lipgloss/v2"
	"github.com/1F47E/rival/internal/config"
	"github.com/1F47E/rival/internal/session"
	"github.com/1F47E/rival/internal/sessionview"
)

// renderDetailMeta renders only the metadata block of the detail view (title,
// fields, error, prompt) plus the "Log" heading. The log body itself lives in a
// scrollable viewport owned by the model, so this function has no log budget
// math — it only clamps itself to height-3 lines so the viewport is always left
// at least three rows to draw in.
func renderDetailMeta(item *displayItem, width, height int, promptExpanded bool, prompts map[string]string) string {
	if item == nil || item.Primary() == nil {
		return labelStyle.Render("Select a session to view details")
	}

	var meta string
	if item.IsGroup() {
		meta = renderGroupDetailMeta(item, width, promptExpanded, prompts)
	} else {
		meta = renderSingleDetailMeta(item.Primary(), width, promptExpanded, prompts)
	}
	return clampMeta(meta, height)
}

// clampMeta truncates the meta block so at least three lines remain for the log
// viewport. When the cut lands inside an expanded prompt the collapsed hint is
// restored so the user still knows there is more text behind "p".
func clampMeta(meta string, height int) string {
	maxMetaLines := height - 3
	if maxMetaLines < 1 {
		maxMetaLines = 1
	}
	lines := renderedLines(meta)
	if len(lines) <= maxMetaLines {
		return meta
	}
	lines = lines[:maxMetaLines]
	hint := labelStyle.Render("... (p to expand)")
	if lines[len(lines)-1] != hint {
		lines[len(lines)-1] = hint
	}
	return strings.Join(lines, "\n")
}

func renderSingleDetailMeta(s *session.Session, width int, promptExpanded bool, prompts map[string]string) string {
	var meta strings.Builder

	id := s.ID
	if len(id) > 8 {
		id = id[:8]
	}
	meta.WriteString(titleStyle.Render(fmt.Sprintf("Session %s", id)))
	meta.WriteString("\n\n")

	// Metadata fields.
	addField(&meta, "Model", config.EngineLabel(s.CLI, s.Model), width)
	addField(&meta, "Effort", s.Effort, width)
	addField(&meta, "Mode", s.Mode, width)
	if s.Account != "" {
		addField(&meta, "Account", s.Account, width)
	}
	addStyledField(&meta, "Status", s.Status, statusStyle(s.Status), width)
	addField(&meta, "WorkDir", s.WorkDir, width)
	addField(&meta, "Started", s.StartTime.Format("15:04:05"), width)
	if s.Duration != "" {
		addField(&meta, "Duration", s.Duration, width)
	}
	if s.ExitCode != nil {
		addField(&meta, "Exit", fmt.Sprintf("%d", *s.ExitCode), width)
	}
	if s.OutputBytes > 0 {
		addField(&meta, "Output", fmt.Sprintf("%d bytes, %d lines", s.OutputBytes, s.OutputLines), width)
	}
	if s.ReviewScope != "" {
		addField(&meta, "Review", s.ReviewScope, width)
	}

	renderErrorSection(&meta, s, width)
	renderPromptSection(&meta, s, width, promptExpanded, prompts)
	meta.WriteString("\n")
	meta.WriteString(titleStyle.Render("Log"))

	return meta.String()
}

func renderGroupDetailMeta(item *displayItem, width int, promptExpanded bool, prompts map[string]string) string {
	s := item.Primary()

	var essential strings.Builder

	id := s.GroupID
	if id == "" {
		id = s.ID
	}
	if len(id) > 8 {
		id = id[:8]
	}
	title := "Megareview"
	switch sessionview.Kind(item.Sessions) {
	case "security":
		title = "Security Review"
	case "antislop":
		title = "Antislop Review"
	case "plan":
		title = "Plan Review"
	}
	essential.WriteString(titleStyle.Render(fmt.Sprintf("%s %s", title, id)))
	essential.WriteString("\n\n")

	// Shared metadata from primary session — derived from the group's sessions so
	// a Sol + Claude plan group is not mislabelled a megareview.
	addField(&essential, "Models", groupCLIs(item), width)
	addField(&essential, "Effort", groupEffort(item), width)
	addField(&essential, "Mode", groupKindLabel(item), width)
	addStyledField(&essential, "Status", groupStatus(item), statusStyle(groupStatus(item)), width)
	addField(&essential, "WorkDir", s.WorkDir, width)
	addField(&essential, "Started", s.StartTime.Format("15:04:05"), width)
	elapsed := groupElapsed(item)
	if elapsed != "-" {
		addField(&essential, "Duration", elapsed, width)
	}
	if s.ReviewScope != "" {
		addField(&essential, "Review", s.ReviewScope, width)
	}

	renderPromptSection(&essential, s, width, promptExpanded, prompts)
	essential.WriteString("\n")
	essential.WriteString(titleStyle.Render("Log"))

	return essential.String()
}

func renderedLines(text string) []string {
	text = strings.TrimRight(text, "\n")
	if text == "" {
		return nil
	}
	return strings.Split(text, "\n")
}

func groupLogLabel(sess *session.Session) string {
	role := "REVIEW"
	if sess.Mode == "consilium" {
		role = "JUDGE"
	}
	label := strings.ToUpper(config.EngineLabel(sess.CLI, sess.Model)) + " " + role
	if sess.Effort != "" {
		label += " · EFFORT " + sess.Effort
	}
	return label
}

// renderErrorSection renders the full error message wrapped across as many
// lines as needed, in the failed (red) style. Unlike a single-line field it is
// never truncated, so long model-runtime errors plus any
// trailing detail stay fully readable.
func renderErrorSection(b *strings.Builder, s *session.Session, width int) {
	if s.ErrorMsg == "" {
		return
	}
	b.WriteString("\n")
	b.WriteString(titleStyle.Render("Error"))
	b.WriteString("\n")
	message := config.PublicRuntimeError(s.CLI, s.Model, s.ErrorMsg)
	for _, line := range wrapText(message, width) {
		b.WriteString(failedStyle.Render(line))
		b.WriteString("\n")
	}
}

// promptFor returns the best prompt text available for a session: the full
// stored prompt when the detail view loaded it, then whatever the record
// carries, then the summary preview.
func promptFor(s *session.Session, prompts map[string]string) string {
	if full, ok := prompts[s.ID]; ok && full != "" {
		return full
	}
	if s.Prompt != "" {
		return s.Prompt
	}
	return s.PromptPreview
}

func renderPromptSection(b *strings.Builder, s *session.Session, width int, promptExpanded bool, prompts map[string]string) {
	prompt := promptFor(s, prompts)
	if prompt == "" {
		return
	}
	b.WriteString("\n")
	b.WriteString(titleStyle.Render("Prompt"))
	b.WriteString("\n")
	promptLines := wrapText(prompt, width)
	if !promptExpanded && len(promptLines) > config.PromptDetailMaxLines {
		for _, line := range promptLines[:config.PromptDetailMaxLines] {
			b.WriteString(line)
			b.WriteString("\n")
		}
		b.WriteString(labelStyle.Render("... (p to expand)"))
		b.WriteString("\n")
	} else {
		for _, line := range promptLines {
			b.WriteString(line)
			b.WriteString("\n")
		}
	}
}

func addField(b *strings.Builder, label, value string, width int) {
	addStyledField(b, label, value, valueStyle, width)
}

func addStyledField(b *strings.Builder, label, value string, style lipgloss.Style, width int) {
	maxValWidth := width - 13
	if maxValWidth < 5 {
		maxValWidth = 5
	}
	rawVal := truncate(value, maxValWidth)
	l := labelStyle.Render(fmt.Sprintf("%-10s", label))
	v := style.Render(rawVal)
	fmt.Fprintf(b, "%s %s\n", l, v)
}

// wrapText word-wraps a string to the given width.
func wrapText(text string, wrapWidth int) []string {
	if wrapWidth <= 0 {
		return []string{text}
	}
	var result []string
	for _, paragraph := range strings.Split(text, "\n") {
		if paragraph == "" {
			result = append(result, "")
			continue
		}
		words := strings.Fields(paragraph)
		if len(words) == 0 {
			result = append(result, "")
			continue
		}
		line := words[0]
		for _, w := range words[1:] {
			if len(line)+1+len(w) > wrapWidth {
				result = append(result, line)
				line = w
			} else {
				line += " " + w
			}
		}
		result = append(result, line)
	}
	return result
}
