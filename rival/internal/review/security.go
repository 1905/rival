package review

import (
	"fmt"
	"strings"

	"github.com/1905/rival/internal/config"
)

// ValidateSecurityResult validates a security payload against the raw output
// it came from. A caller must treat a validation failure exactly like a parse
// failure: `{"summary":"","findings":null}` parses cleanly, and the prompt
// carries a parseable clean example, so an empty payload or an echoed prompt
// would otherwise format as "no vulnerabilities" when nothing was reviewed.
func ValidateSecurityResult(out *ReviewerOutput, raw string) error {
	if err := validateOutput(out, raw, securityEcho); err != nil {
		return err
	}
	for i, f := range out.Findings {
		if strings.TrimSpace(f.File) == "" {
			return fmt.Errorf("finding %d has no file", i+1)
		}
		if strings.TrimSpace(f.Title) == "" {
			return fmt.Errorf("finding %d has no title", i+1)
		}
		if !knownSeverity(f.Severity) {
			return fmt.Errorf("finding %d has an unknown severity %q", i+1, f.Severity)
		}
	}
	return nil
}

// promptEchoMarkers are phrases from the security prompt. Security runs on
// opencode, whose log never contains the prompt, so a marker in a clean
// security result means the model echoed its instructions. A false
// "unusable" is the safe failure for a security gate, so this stays strict.
var promptEchoMarkers = []string{
	"## Role: Security Reviewer",
	"Work through every class below",
}

// isCleanExample reports whether out is exactly the prompt's clean example.
func isCleanExample(out *ReviewerOutput) bool {
	return strings.TrimSpace(out.Summary) == "No issues found." && len(out.Findings) == 0
}

// securityEcho is the strict echo check for security reviews.
func securityEcho(out *ReviewerOutput, raw string) bool {
	if !isCleanExample(out) {
		return false
	}
	for _, marker := range promptEchoMarkers {
		if strings.Contains(raw, marker) {
			return true
		}
	}
	return false
}

// bugHunterEcho is the echo check for bug-hunter reviews. Codex writes the
// whole prompt into its log, and a reviewer may quote prompt.go (security
// markers included) in tool output, so no marker is evidence. The output is
// an echo only when no reviewer payload follows the last copy of the
// prompt's clean example.
func bugHunterEcho(out *ReviewerOutput, raw string) bool {
	if !isCleanExample(out) {
		return false
	}
	i := strings.LastIndex(raw, cleanReviewExampleLine)
	if i < 0 {
		return false
	}
	_, err := ParseReviewerOutput(raw[i+len(cleanReviewExampleLine):])
	return err != nil
}

// FormatSecurityResult renders a security review, or falls back to the raw log
// when the payload is unusable. It owns that choice because the inner
// formatter has neither the raw output nor the CLI the normalizer needs. The
// returned error is the validation failure: a security gate must not exit 0
// on output it cannot trust.
func FormatSecurityResult(parsed *ReviewerOutput, raw, cli, model, scope, logPath string) (string, error) {
	label := config.EngineLabel(cli, model)
	if err := ValidateSecurityResult(parsed, raw); err != nil {
		return formatUnusable("RIVAL SECURITY REVIEW — UNUSABLE OUTPUT", label, scope, err,
			"The model ran but did not return a review this can trust.\nRaw output follows for diagnosis.\n\n",
			config.PublicRuntimeLog(cli, model, raw), logPath), err
	}
	return FormatSecurityConsole(parsed, label, scope), nil
}

// FormatSecurityConsole renders a validated security review.
func FormatSecurityConsole(out *ReviewerOutput, model, scope string) string {
	var sb strings.Builder
	writeHeader(&sb, "RIVAL SECURITY REVIEW", model, scope, out.Summary)

	if len(out.Findings) == 0 {
		sb.WriteString("No vulnerabilities found.\n")
		return sb.String()
	}

	findings := sortedFindings(out.Findings)
	for i, f := range findings {
		writeFinding(&sb, i+1, f)
	}
	sb.WriteString(severityTally(findings) + "\n")
	return sb.String()
}
