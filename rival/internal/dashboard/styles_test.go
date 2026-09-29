package dashboard

import (
	"regexp"
	"strings"
	"testing"

	"charm.land/lipgloss/v2"
)

var truecolorFg = regexp.MustCompile(`38;2;\d+;\d+;\d+`)

func TestRenderLogoAppliesGradientAndKeepsWidth(t *testing.T) {
	logo := renderLogo()
	lines := strings.Split(logo, "\n")
	if len(lines) != len(bannerLines) {
		t.Fatalf("logo has %d lines, want %d", len(lines), len(bannerLines))
	}
	for i, line := range lines {
		if w := lipgloss.Width(line); w != bannerWidth {
			t.Errorf("logo line %d width = %d, want %d: %q", i, w, bannerWidth, line)
		}
	}
	distinct := map[string]bool{}
	for _, seq := range truecolorFg.FindAllString(logo, -1) {
		distinct[seq] = true
	}
	if len(distinct) < 3 {
		t.Fatalf("logo carries %d distinct truecolor sequences, want >= 3 (gradient not applied)", len(distinct))
	}
	if again := renderLogo(); again != logo {
		t.Fatal("second renderLogo call returned a different string; the logo must be cached")
	}
}

func TestRenderHeaderWide(t *testing.T) {
	st := headerStats{Running: 1, Queued: 7, Completed: 2684, Failed: 296, Total: 2988, Version: "3.34.0"}
	got := renderHeader(200, false, st, "⠋")
	lines := strings.Split(got, "\n")
	if len(lines) != len(bannerLines) {
		t.Fatalf("wide header has %d lines, want %d:\n%s", len(lines), len(bannerLines), got)
	}
	for i, line := range lines {
		if w := lipgloss.Width(line); w > 200 {
			t.Errorf("header line %d width %d exceeds 200", i, w)
		}
	}
	for _, want := range []string{"3.34.0", "1", "7", "2684", "296"} {
		if !strings.Contains(got, want) {
			t.Errorf("wide header omits %q:\n%s", want, got)
		}
	}
}

func TestRenderHeaderCompactIsOneLine(t *testing.T) {
	st := headerStats{Running: 1, Completed: 2684, Failed: 296, Total: 2981, Version: "3.34.0"}
	got := renderHeader(80, true, st, "⠋")
	if strings.Contains(got, "\n") {
		t.Fatalf("compact header spans several lines:\n%s", got)
	}
	if w := lipgloss.Width(got); w > 80 {
		t.Fatalf("compact header width %d exceeds 80", w)
	}
	for _, want := range []string{"2684", "296", "3.34.0"} {
		if !strings.Contains(got, want) {
			t.Errorf("compact header omits %q: %q", want, got)
		}
	}
}

func TestRenderHeaderNeverExceedsNarrowWidth(t *testing.T) {
	st := headerStats{Running: 12, Queued: 3, Completed: 123456, Failed: 7890, Total: 131361, Version: "3.34.0-dirty"}
	for _, compact := range []bool{false, true} {
		for _, width := range []int{59, 40, 20} {
			for i, line := range strings.Split(renderHeader(width, compact, st, "⠋"), "\n") {
				if w := lipgloss.Width(line); w > width {
					t.Errorf("compact=%v width=%d: line %d is %d cells", compact, width, i, w)
				}
			}
		}
	}
}

func TestStatusStyleColoursEachStatus(t *testing.T) {
	seen := map[string]string{}
	for _, status := range []string{"running", "queued", "completed", "failed"} {
		out := statusStyle(status).Render("x")
		if out == "x" {
			t.Errorf("status %q is unstyled", status)
		}
		if prev, dup := seen[out]; dup {
			t.Errorf("status %q renders like %q", status, prev)
		}
		seen[out] = status
	}
}
