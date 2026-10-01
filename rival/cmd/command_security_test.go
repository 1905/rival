package cmd

import (
	"strings"
	"testing"

	"github.com/1905/rival/internal/config"
	"github.com/1905/rival/internal/review"
)

// The security command must never borrow the bug-hunter prompt: that would
// run a bug hunt on the default empty-stdin invocation while reporting a
// security review.
func TestSecurityPromptIsAlwaysTheSecurityLens(t *testing.T) {
	bugHunter := review.BuildReviewerPrompt("x", config.PromptBugHunter)
	securityMarker := "## Role: Security Reviewer"

	for _, tc := range []struct {
		name  string
		scope string
	}{
		{"explicit scope", "src/api/"},
		{"empty scope falls back to git or whole project", ""},
	} {
		t.Run(tc.name, func(t *testing.T) {
			prompt, scope := securityScopeAndPrompt(tc.scope, t.TempDir())
			if !strings.Contains(prompt, securityMarker) {
				t.Errorf("prompt is not the security lens:\n%s", prompt[:200])
			}
			if strings.Contains(prompt, "## Role: Implementation Bug Hunter") {
				t.Error("the bug-hunter prompt leaked into a security run")
			}
			if strings.Contains(prompt, bugHunter[:80]) {
				t.Error("prompt starts with the bug-hunter text")
			}
			if scope == "" {
				t.Error("scope is empty")
			}
		})
	}
}

// A temp dir is not a git repo, so auto-scope falls back to the whole project.
func TestSecurityAutoScopeFallsBackToWholeProject(t *testing.T) {
	_, scope := securityScopeAndPrompt("", t.TempDir())
	if scope != "the entire project" {
		t.Errorf("scope = %q, want the whole-project fallback", scope)
	}
}

func TestSecurityScopeIsCarriedIntoThePrompt(t *testing.T) {
	prompt, scope := securityScopeAndPrompt("internal/auth/", t.TempDir())
	if scope != "internal/auth/" {
		t.Errorf("scope = %q", scope)
	}
	if !strings.Contains(prompt, "internal/auth/") {
		t.Error("the scope never reached the prompt")
	}
}

// The twelve classes are the taxonomy the plan reviews settled on. A prompt
// that quietly loses one produces a review that looks complete while never
// checking for it.
func TestSecurityPromptCoversEveryVulnerabilityClass(t *testing.T) {
	prompt := review.BuildReviewerPrompt("src/", config.PromptSecurity)
	for _, marker := range []string{
		"njection", "uthorization", "uthentication", "rypto", "raversal",
		"SSRF", "eserializ", "ecret", "alidation", "CSRF", "edirect", "xhaustion",
	} {
		if !strings.Contains(prompt, marker) {
			t.Errorf("the security prompt does not cover %q", marker)
		}
	}
}

// The bug-hunter prompt must be untouched by adding a second lens.
func TestBugHunterPromptUnchangedByTheSecurityLens(t *testing.T) {
	prompt := review.BuildReviewerPrompt("src/", config.PromptBugHunter)
	if !strings.Contains(prompt, "## Role: Implementation Bug Hunter") {
		t.Error("the bug-hunter prompt changed")
	}
	if strings.Contains(prompt, "## Role: Security Reviewer") {
		t.Error("the security prompt leaked into a bug-hunter run")
	}
}

// A security payload printed by a tool (a file the model read) must not pass
// for the review: only the final answer after the last "codex" header counts,
// so unstructured prose there is unusable output, not the tool's findings.
func TestSecurityParsesFinalAnswerNotToolOutput(t *testing.T) {
	raw := "user\nreview src/\n" +
		"exec\ncat old-security.json\n" +
		`{"summary": "One injection.", "findings": [{"file": "a.go", "line": 3, "severity": "high", "category": "security", "title": "sql injection", "body": "concatenated query", "suggestion": "bind params", "confidence": 9}]}` + "\n" +
		"codex\nI looked around; nothing jumped out.\n"
	out, err := formatSecurityOutput(raw, config.KimiModel, "src/", "/tmp/x.log")
	if err == nil {
		t.Fatalf("tool output accepted as the security review:\n%s", out)
	}
}
