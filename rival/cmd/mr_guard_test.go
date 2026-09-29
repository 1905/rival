package cmd

import (
	"strings"
	"testing"
)

// A raw prompt cannot pin an MR checkout, so an MR URL in one is rejected
// before any reviewer starts, with a pointer to the review path that can.
func TestModelCommandRejectsMRInRawPrompt(t *testing.T) {
	_, calls := fakeMR(t)
	f := &fakeRun{}
	_, err := runCommandWith(t, f, "сделай ревью МР "+testMRURL, t.TempDir())
	if f.called || *calls != 0 {
		t.Fatalf("reviewer or MR resolver reached: run=%v resolver=%d", f.called, *calls)
	}
	if err == nil || !strings.Contains(err.Error(), "no reviewer was started") ||
		!strings.Contains(err.Error(), "rival command codex review <MR-URL>") {
		t.Fatalf("err = %v, want a rejection pointing to rival command codex review <MR-URL>", err)
	}
}
