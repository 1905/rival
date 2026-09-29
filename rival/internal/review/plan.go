package review

import (
	"encoding/json"
	"fmt"
	"strings"

	"github.com/1905/rival/internal/config"
)

// PlanOutput is the structured JSON emitted by `rival command plan`. It mirrors
// ReviewerOutput but carries a 1-10 Rating and is parsed independently so the
// plan command stays decoupled from the code-review types.
type PlanOutput struct {
	Summary  string            `json:"summary"`
	Rating   int               `json:"rating"`
	Findings []ReviewerFinding `json:"findings"`
}

// planExampleSummary is the exact summary string from the PlanReviewPrompt schema
// example. A real plan review never emits this verbatim, so it lets us skip the
// echoed schema and pick the model's real answer.
const planExampleSummary = "1-3 sentence overall assessment of the plan"

// ParsePlanOutput extracts the plan reviewer's structured JSON from raw CLI
// output. Like ParseReviewerOutput, the CLI echoes the prompt (with the schema
// example) and prints unrelated JSON, so we scan every top-level JSON object and
// take the LAST genuine plan payload — it must carry both "findings" and "rating"
// keys and must not be the schema example itself.
func ParsePlanOutput(raw string) (*PlanOutput, error) {
	objs := jsonObjects(raw)
	var lastErr error
	for i := len(objs) - 1; i >= 0; i-- {
		c := objs[i]
		// A genuine plan payload carries summary + rating + findings. Requiring all
		// three rejects unrelated/tool JSON and reviewer (non-plan) payloads.
		if !hasJSONKey(c, "findings") || !hasJSONKey(c, "rating") || !hasJSONKey(c, "summary") {
			continue
		}
		var out PlanOutput
		if err := json.Unmarshal([]byte(c), &out); err != nil {
			lastErr = err
			continue
		}
		if strings.TrimSpace(out.Summary) == planExampleSummary {
			continue // the echoed schema example, not a real answer
		}
		if out.Rating < 1 || out.Rating > 10 {
			continue // not a real 1-10 plan rating; likely noise or a partial echo
		}
		out.Findings = dropPlaceholderReviewerFindings(out.Findings)
		return &out, nil
	}
	if lastErr != nil {
		return nil, fmt.Errorf("no valid plan JSON payload (last decode error: %w)", lastErr)
	}
	return nil, fmt.Errorf("no plan JSON payload found in output")
}

// FormatPlanConsole renders a single-CLI PlanOutput for the caller: the header,
// the file, the 1-10 rating, the summary, then every finding grouped by severity
// bucket (crit→high→med→low) and numbered globally 1..N. No confidence filtering —
// all findings are returned.
func FormatPlanConsole(out *PlanOutput, file string) string {
	var sb strings.Builder
	sb.WriteString("\n═══ RIVAL PLAN REVIEW ═══\n\n")
	fmt.Fprintf(&sb, "File: %s\n", file)
	formatPlanBody(out, &sb, planRatingLabel, planEmptyLine)
	return sb.String()
}

// Body labels for the two review flavors sharing formatPlanBody.
const (
	planRatingLabel     = "Rating"
	planEmptyLine       = "No bugs or gaps found."
	antislopRatingLabel = "Leanness"
	antislopEmptyLine   = "No slop found."
)

// formatPlanBody writes the rating/summary/findings/tally for one PlanOutput into
// sb. It carries no header or File line, so it is shared by the single-CLI
// (FormatPlanConsole), multi-model (FormatPlanMultiConsole), and antislop
// renderers; ratingLabel and emptyLine are the only flavor-specific strings.
func formatPlanBody(out *PlanOutput, sb *strings.Builder, ratingLabel, emptyLine string) {
	fmt.Fprintf(sb, "%s: %d/10\n\n", ratingLabel, out.Rating)
	writeSummary(sb, out.Summary)

	// Sorted by severity (crit first), then confidence (highest first), so
	// findings come out grouped without a separate bucketing pass.
	findings := sortedFindings(out.Findings)

	if len(findings) == 0 {
		sb.WriteString(emptyLine + "\n")
		return
	}

	for i, f := range findings {
		// The plan/antislop schema has no failure_scenario; drop one a model
		// volunteers so doc reviews never print a Scenario line.
		f.FailureScenario = ""
		writeFinding(sb, i+1, f)
	}

	// Severity tally for a quick read.
	sb.WriteString(severityTally(findings) + "\n")
}

// FormatPlanResult renders a PlanRunResult for the caller. A single successful
// model reuses the original single-model layout (FormatPlanConsole when parsed,
// else the raw output). Two or more models use the multi-block layout. Any
// skipped models are surfaced.
func FormatPlanResult(result *PlanRunResult, file string) string {
	if result == nil || len(result.Results) == 0 {
		return "No plan review output.\n"
	}
	if len(result.Results) == 1 && len(result.Skipped) == 0 {
		r := result.Results[0]
		if r.Parsed != nil {
			return FormatPlanConsole(r.Parsed, file)
		}
		// Parse failed — preserve the raw model output while normalizing any
		// adapter banner to the concrete model name.
		return config.PublicRuntimeLog(r.CLI, r.Model, r.Raw)
	}
	return FormatPlanMultiConsole(result.Results, result.Skipped, file)
}

// FormatAntislopResult renders a PlanRunResult from an antislop run. Same
// dispatch as FormatPlanResult with the antislop header and body labels.
func FormatAntislopResult(result *PlanRunResult, target string) string {
	targetLine := fmt.Sprintf("Scope: %s", target)

	if result == nil || len(result.Results) == 0 {
		return "No antislop review output.\n"
	}
	if len(result.Results) == 1 && len(result.Skipped) == 0 {
		r := result.Results[0]
		if r.Parsed == nil {
			return config.PublicRuntimeLog(r.CLI, r.Model, r.Raw)
		}
		var sb strings.Builder
		sb.WriteString("\n═══ RIVAL ANTISLOP REVIEW ═══\n\n")
		sb.WriteString(targetLine + "\n")
		formatPlanBody(r.Parsed, &sb, antislopRatingLabel, antislopEmptyLine)
		return sb.String()
	}
	return formatDocMultiConsole(result.Results, result.Skipped, "RIVAL ANTISLOP REVIEW", targetLine, antislopRatingLabel, antislopEmptyLine)
}

// PlanCLIResult is one CLI's plan-review outcome. Parsed is nil when the CLI's
// output could not be parsed into structured JSON, in which case Raw holds the
// unparsed CLI output so nothing the model produced is lost.
type PlanCLIResult struct {
	CLI    string
	Model  string
	Parsed *PlanOutput
	Raw    string
}

// FormatPlanMultiConsole renders 2+ models' plan reviews as separate labelled
// blocks under one header, followed by any skipped models. A result whose Parsed
// is nil falls back to printing its Raw output so a parse failure never drops
// the model's work.
func FormatPlanMultiConsole(results []PlanCLIResult, skipped []SkippedCLI, file string) string {
	return formatDocMultiConsole(results, skipped, "RIVAL PLAN REVIEW", "File: "+file, planRatingLabel, planEmptyLine)
}

// formatDocMultiConsole is the multi-model renderer shared by plan and antislop
// reviews; headerName, targetLine, and the body labels are the only differences.
func formatDocMultiConsole(results []PlanCLIResult, skipped []SkippedCLI, headerName, targetLine, ratingLabel, emptyLine string) string {
	var sb strings.Builder

	labels := make([]string, 0, len(results))
	for _, r := range results {
		labels = append(labels, config.EngineLabel(r.CLI, r.Model))
	}
	fmt.Fprintf(&sb, "\n═══ %s (%s) ═══\n\n", headerName, strings.Join(labels, " + "))
	sb.WriteString(targetLine + "\n")

	for i, r := range results {
		fmt.Fprintf(&sb, "\n── %s ──\n\n", config.EngineLabel(r.CLI, r.Model))
		if r.Parsed != nil {
			formatPlanBody(r.Parsed, &sb, ratingLabel, emptyLine)
		} else {
			// Parse failed — emit normalized raw output so nothing is lost while
			// internal runtime identifiers stay out of the public result.
			sb.WriteString("(could not parse structured output — raw output below)\n\n")
			sb.WriteString(strings.TrimSpace(config.PublicRuntimeLog(r.CLI, r.Model, r.Raw)))
			sb.WriteString("\n")
		}
		if i < len(results)-1 {
			sb.WriteString("\n")
		}
	}

	if len(skipped) > 0 {
		sb.WriteString("\n")
		for _, s := range skipped {
			reason := config.PublicRuntimeError(s.CLI, s.Model, s.Reason)
			fmt.Fprintf(&sb, "Skipped: %s — %s\n", s.Label(), reason)
		}
	}

	return sb.String()
}
