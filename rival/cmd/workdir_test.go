package cmd

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestResolveWorkdir(t *testing.T) {
	root := t.TempDir()
	if err := os.Mkdir(filepath.Join(root, "v3"), 0o700); err != nil {
		t.Fatal(err)
	}
	file := filepath.Join(root, "plain.txt")
	if err := os.WriteFile(file, []byte("x"), 0o600); err != nil {
		t.Fatal(err)
	}
	t.Chdir(root)
	cwd, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}

	tests := []struct {
		name    string
		raw     string
		want    string
		wantErr string
	}{
		{name: "relative subdir with trailing slash", raw: "v3/", want: filepath.Join(cwd, "v3")},
		{name: "dot", raw: ".", want: cwd},
		{name: "unclean relative", raw: "./v3/../v3", want: filepath.Join(cwd, "v3")},
		{name: "absolute", raw: filepath.Join(root, "v3"), want: filepath.Join(root, "v3")},
		{name: "missing", raw: "nope", wantErr: "workdir not found: " + filepath.Join(cwd, "nope")},
		{name: "file", raw: "plain.txt", wantErr: "workdir is not a directory: " + filepath.Join(cwd, "plain.txt")},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			got, err := resolveWorkdir(tc.raw)
			if tc.wantErr != "" {
				if err == nil || !strings.Contains(err.Error(), tc.wantErr) {
					t.Fatalf("resolveWorkdir(%q) err = %v, want %q", tc.raw, err, tc.wantErr)
				}
				return
			}
			if err != nil {
				t.Fatalf("resolveWorkdir(%q): %v", tc.raw, err)
			}
			if got != tc.want {
				t.Errorf("resolveWorkdir(%q) = %q, want %q", tc.raw, got, tc.want)
			}
		})
	}
}
