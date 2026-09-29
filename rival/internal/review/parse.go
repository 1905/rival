package review

import (
	"encoding/json"
	"fmt"
	"regexp"
	"strings"
)

// ParseReviewerOutput extracts a reviewer's structured JSON from raw CLI output.
//
// CLIs make this more than one json.Unmarshal: they echo the prompt (which
// contains the schema example), print unrelated JSON (tool/telemetry events),
// and wrap JSON in prose. We therefore scan every top-level JSON object and pick
// the LAST one that is a genuine reviewer payload — it must carry both "summary"
// and "findings" keys and must not be the schema example itself.
func ParseReviewerOutput(raw string) (*ReviewerOutput, error) {
	objs := jsonObjects(raw)
	var lastErr error
	for i := len(objs) - 1; i >= 0; i-- {
		c := objs[i]
		if !hasJSONKey(c, "summary") || !hasJSONKey(c, "findings") {
			continue
		}
		var out ReviewerOutput
		if err := json.Unmarshal([]byte(c), &out); err != nil {
			lastErr = err // a payload-shaped candidate that failed to decode
			continue
		}
		if isExampleSummary(out.Summary) {
			continue // the echoed schema example, not a real answer
		}
		out.Findings = dropPlaceholderReviewerFindings(out.Findings)
		return &out, nil
	}
	if lastErr != nil {
		return nil, fmt.Errorf("no valid reviewer JSON payload (last decode error: %w)", lastErr)
	}
	return nil, fmt.Errorf("no reviewer JSON payload found in output")
}

// hasJSONKey reports whether candidate is a JSON object with the given top-level
// key actually present (not merely defaulting to a zero value on unmarshal).
// This rejects unrelated JSON such as a {"event":"done"} tool/telemetry line.
func hasJSONKey(candidate, key string) bool {
	var m map[string]json.RawMessage
	if err := json.Unmarshal([]byte(candidate), &m); err != nil {
		return false
	}
	_, ok := m[key]
	return ok
}

// isExampleSummary matches the schema example summary from the prompt contract
// (reviewerJSONContract). A real summary is never this exact string. The
// clean-review example uses a real sentence ("No issues found.") so a genuine
// clean review is not mistaken for the example.
func isExampleSummary(s string) bool {
	return strings.TrimSpace(s) == "1-3 sentence reviewer summary"
}

// isPlaceholderFinding matches a finding copied from the schema example: the enum
// fields hold the literal pipe-delimited option lists, or the file is the
// contract's placeholder. Real findings never have these field values. The
// category is compared against every prompt contract's exact enum literal
// (code review, plan, antislop) — exact matches only, because models under
// uncertainty emit real dual categories like "bug|security" that a looser
// pipe check would silently drop.
func isPlaceholderFinding(file, severity, category string) bool {
	switch category {
	case "bug|security|performance|concurrency|architecture|tests|ux",
		"bug|gap|ambiguity|scope|verification",
		"reuse|simplify|efficiency|altitude|compat|reinvention|slop|yagni":
		return true
	}
	return file == "path/to/file" ||
		severity == "critical|high|medium|low"
}

// dropPlaceholderReviewerFindings removes individual schema-example findings so a
// single echoed placeholder item does not discard an otherwise real review.
func dropPlaceholderReviewerFindings(in []ReviewerFinding) []ReviewerFinding {
	out := in[:0:0]
	for _, f := range in {
		if isPlaceholderFinding(f.File, f.Severity, f.Category) {
			continue
		}
		out = append(out, f)
	}
	return out
}

// jsonObjects returns every balanced, valid JSON object in s, in order of
// appearance (closing-brace order), including objects nested inside larger
// non-JSON brace spans.
//
// It is a single forward pass using an explicit stack of '{' indices (no
// recursion, so adversarial deep nesting can't overflow the goroutine stack;
// and no per-brace re-scan, so it stays linear rather than O(N^2)). Every time
// a brace closes we test the span it delimits with json.Valid, so:
//   - a lone/unbalanced brace from a grep/tool line never hides a later payload
//     (the payload's own brace pair is still tested when it closes), and
//   - a valid payload nested inside a balanced-but-invalid span is still found.
//
// While outside any object (empty stack) prose and code are ignored entirely —
// only '{' matters — so stray quotes or braces in surrounding text can't desync
// the scan. JSON string/escape state is tracked only inside an object.
func jsonObjects(s string) []string {
	var out []string
	var stack []int // indices of currently-open '{'
	inStr := false
	esc := false

	for i := 0; i < len(s); i++ {
		c := s[i]

		if len(stack) == 0 {
			if c == '{' {
				stack = append(stack, i)
				inStr = false
				esc = false
			}
			continue
		}

		if esc {
			esc = false
			continue
		}
		if c == '\\' {
			if inStr {
				esc = true
			}
			continue
		}
		if c == '"' {
			inStr = !inStr
			continue
		}
		if inStr {
			continue
		}
		switch c {
		case '{':
			stack = append(stack, i)
		case '}':
			start := stack[len(stack)-1]
			stack = stack[:len(stack)-1]
			if candidate := s[start : i+1]; json.Valid([]byte(candidate)) {
				out = append(out, candidate)
			}
		}
	}
	return out
}

// codexAnswerHeader is the line codex prints before the assistant's final
// message. Everything above it is the echoed prompt and tool output.
var codexAnswerHeader = regexp.MustCompile(`(?m)^codex$`)

// FinalAnswer returns the part of a provider log that holds the model's final
// answer. For codex that is the text after the last "codex" header line, so
// review-shaped JSON a tool printed earlier (a file the model read) can never
// be taken for the review. Logs without that header are returned whole.
func FinalAnswer(raw string) string {
	locs := codexAnswerHeader.FindAllStringIndex(raw, -1)
	if len(locs) == 0 {
		return raw
	}
	return raw[locs[len(locs)-1][1]:]
}
