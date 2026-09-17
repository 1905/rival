package cmd

import (
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/1F47E/rival/internal/parser"
)

func TestModelCommandRejectsMRBeforeReviewer(t *testing.T) {
	for _, raw := range []string{
		"сделай ревью МР https://gitlab.example.com/team/app/-/merge_requests/42",
		"review https://gitlab.example.com/team/app/-/merge_requests/42",
	} {
		t.Run(raw, func(t *testing.T) {
			path := filepath.Join(t.TempDir(), "input")
			if err := os.WriteFile(path, []byte(raw), 0o600); err != nil {
				t.Fatal(err)
			}
			input, err := os.Open(path)
			if err != nil {
				t.Fatal(err)
			}
			defer func() { _ = input.Close() }()
			previous := os.Stdin
			os.Stdin = input
			defer func() { os.Stdin = previous }()
			called := false
			spec := solSpec()
			spec.parse = parser.ParseGPT56SolArgs
			spec.preflight = func(string) error {
				called = true
				return errors.New("reviewer reached without resolving the MR")
			}
			err = runModelCommand(spec, t.TempDir(), true)
			if called || err == nil || !strings.Contains(err.Error(), "rival review") {
				t.Fatalf("MR was not rejected before reviewer: called=%v err=%v", called, err)
			}
		})
	}
}
