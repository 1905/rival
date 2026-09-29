package review

import (
	"fmt"
	"sort"
	"strings"

	"github.com/1905/rival/internal/config"
)

// DefaultConfidenceThreshold splits a single-model review: findings below it
// go in a short "Low confidence" block instead of the main list.
const DefaultConfidenceThreshold = 6

// severities is the canonical severity ladder, most severe first, with the
// short label each one displays as.
var severities = []struct{ name, short string }{
	{"critical", "crit"},
	{"high", "high"},
	{"medium", "med"},
	{"low", "low"},
}

// severityRank orders severities critical-first. An unknown severity ranks
// len(severities), after every known one.
func severityRank(s string) int {
	s = strings.ToLower(s)
	for i, sev := range severities {
		if sev.name == s {
			return i
		}
	}
	return len(severities)
}

// knownSeverity reports whether s is one of the canonical severities.
func knownSeverity(s string) bool {
	return severityRank(s) < len(severities)
}

// displaySeverity maps a canonical severity to its short label
// (crit/high/med/low) in the review, security and plan output. Unknown values
// pass through lowercased.
func displaySeverity(s string) string {
	if rank := severityRank(s); rank < len(severities) {
		return severities[rank].short
	}
	return strings.ToLower(s)
}

// sortedFindings returns a copy of in ordered by severity (critical first),
// then confidence (highest first). The sort is stable, so equal findings keep
// the model's order. Shared by the review, security and plan renderers.
func sortedFindings(in []ReviewerFinding) []ReviewerFinding {
	out := make([]ReviewerFinding, len(in))
	copy(out, in)
	sort.SliceStable(out, func(i, j int) bool {
		ri, rj := severityRank(out[i].Severity), severityRank(out[j].Severity)
		if ri != rj {
			return ri < rj
		}
		return out[i].Confidence > out[j].Confidence
	})
	return out
}

// severityTally returns the one-line count, without a trailing newline. An
// unknown severity counts as low.
func severityTally(fs []ReviewerFinding) string {
	counts := make([]int, len(severities))
	for _, f := range fs {
		counts[min(severityRank(f.Severity), len(severities)-1)]++
	}
	parts := make([]string, len(severities))
	for i, sev := range severities {
		parts[i] = fmt.Sprintf("%d %s", counts[i], sev.short)
	}
	return fmt.Sprintf("Findings: %d total — %s", len(fs), strings.Join(parts, ", "))
}

// findingLocation is "file:line", or just the file when the line is unknown.
func findingLocation(f ReviewerFinding) string {
	if f.Line > 0 {
		return fmt.Sprintf("%s:%d", f.File, f.Line)
	}
	return f.File
}

// writeFinding renders one numbered finding block, followed by a blank line.
// The review, security and plan renderers share this exact layout.
func writeFinding(sb *strings.Builder, n int, f ReviewerFinding) {
	fmt.Fprintf(sb, "%d. [%s] %s", n, displaySeverity(f.Severity), f.Title)
	if loc := findingLocation(f); loc != "" {
		fmt.Fprintf(sb, " — %s", loc)
	}
	sb.WriteString("\n")
	if body := strings.TrimSpace(f.Body); body != "" {
		fmt.Fprintf(sb, "   %s\n", body)
	}
	if scenario := strings.TrimSpace(f.FailureScenario); scenario != "" {
		fmt.Fprintf(sb, "   Scenario: %s\n", scenario)
	}
	if fix := strings.TrimSpace(f.Suggestion); fix != "" {
		fmt.Fprintf(sb, "   Fix: %s\n", fix)
	}
	if f.Category != "" {
		fmt.Fprintf(sb, "   (%s, confidence %d)\n", f.Category, f.Confidence)
	} else {
		fmt.Fprintf(sb, "   (confidence %d)\n", f.Confidence)
	}
	sb.WriteString("\n")
}

// validateOutput reports whether a parsed payload is a usable review.
// ParseReviewerOutput only checks that the JSON decodes with the right keys,
// so nil, an echoed prompt example and an empty summary all get here looking
// like a clean review. echo is the lens's own echo check.
func validateOutput(out *ReviewerOutput, raw string, echo func(*ReviewerOutput, string) bool) error {
	if out == nil {
		return fmt.Errorf("no structured output")
	}
	if strings.TrimSpace(out.Summary) == "" {
		return fmt.Errorf("summary is empty")
	}
	if raw != "" && echo(out, raw) {
		return fmt.Errorf("output repeats the prompt's own example rather than a review")
	}
	return nil
}

// validateReviewResult validates a bug-hunter review.
func validateReviewResult(out *ReviewerOutput, raw string) error {
	return validateOutput(out, raw, bugHunterEcho)
}

// FormatReviewResult renders a single-model review, or the raw log when the
// payload is unusable. modelLabel in the output is "<label> (<model id>)".
func FormatReviewResult(parsed *ReviewerOutput, raw, cli, model, scope, logPath string) string {
	label := config.EngineLabel(cli, model) + " (" + model + ")"
	if err := validateReviewResult(parsed, raw); err != nil {
		return formatUnusable("RIVAL REVIEW — UNPARSED OUTPUT", label, scope, err,
			"The model ran but its answer is not a structured review.\nRaw output follows.\n\n",
			config.PublicRuntimeLog(cli, model, raw), logPath)
	}
	return FormatReviewConsole(parsed, label, scope, logPath)
}

// formatUnusable renders a run whose output cannot be trusted as a review:
// the header, the problem, an explanation, then the public raw log so nothing
// is lost. It ends with the log path when one is known.
func formatUnusable(title, label, scope string, problem error, explain, public, logPath string) string {
	var sb strings.Builder
	fmt.Fprintf(&sb, "\n═══ %s ═══\n\n", title)
	fmt.Fprintf(&sb, "Model: %s\nScope: %s\nProblem: %s\n\n", label, oneLine(scope), problem)
	sb.WriteString(explain)
	sb.WriteString(public)
	if public != "" && !strings.HasSuffix(public, "\n") {
		sb.WriteString("\n")
	}
	if logPath != "" {
		fmt.Fprintf(&sb, "\nLog: %s\n", logPath)
	}
	return sb.String()
}

// FormatReviewConsole renders a validated single-model review: findings at or
// above DefaultConfidenceThreshold in full, the rest in a short block.
func FormatReviewConsole(out *ReviewerOutput, modelLabel, scope, logPath string) string {
	var sb strings.Builder
	writeHeader(&sb, "RIVAL REVIEW", modelLabel, scope, out.Summary)

	if len(out.Findings) == 0 {
		sb.WriteString("No issues found.\n")
	} else {
		var main, low []ReviewerFinding
		for _, f := range sortedFindings(out.Findings) {
			if f.Confidence < DefaultConfidenceThreshold {
				low = append(low, f)
			} else {
				main = append(main, f)
			}
		}
		if len(main) == 0 {
			fmt.Fprintf(&sb, "No findings at confidence %d or higher.\n\n", DefaultConfidenceThreshold)
		}
		for i, f := range main {
			writeFinding(&sb, i+1, f)
		}
		if len(low) > 0 {
			fmt.Fprintf(&sb, "Low confidence (%d):\n", len(low))
			for _, f := range low {
				fmt.Fprintf(&sb, "- [%s] %s", displaySeverity(f.Severity), f.Title)
				if loc := findingLocation(f); loc != "" {
					fmt.Fprintf(&sb, " — %s", loc)
				}
				fmt.Fprintf(&sb, " (confidence %d)\n", f.Confidence)
			}
			sb.WriteString("\n")
		}
		sb.WriteString(severityTally(main) + "\n")
	}

	if logPath != "" {
		fmt.Fprintf(&sb, "Log: %s\n", logPath)
	}
	return sb.String()
}

// writeHeader writes the review and security console header: the title, the
// Model and Scope lines, then the summary.
func writeHeader(sb *strings.Builder, title, label, scope, summary string) {
	fmt.Fprintf(sb, "\n═══ %s ═══\n\n", title)
	fmt.Fprintf(sb, "Model: %s\n", label)
	fmt.Fprintf(sb, "Scope: %s\n\n", oneLine(scope))
	writeSummary(sb, summary)
}

// writeSummary writes the Summary line and a blank line, or nothing when the
// summary is blank.
func writeSummary(sb *strings.Builder, summary string) {
	if s := strings.TrimSpace(summary); s != "" {
		fmt.Fprintf(sb, "Summary: %s\n\n", s)
	}
}

// oneLine joins a multi-line scope (the auto-detected changed-file list) so
// the Scope line stays one line.
func oneLine(s string) string {
	var parts []string
	for _, line := range strings.Split(s, "\n") {
		if line = strings.TrimSpace(line); line != "" {
			parts = append(parts, line)
		}
	}
	return strings.Join(parts, ", ")
}
