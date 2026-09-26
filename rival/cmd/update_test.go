package cmd

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/spf13/cobra"
)

func TestUpdateInstallsFromSelectedBinary(t *testing.T) {
	binary := filepath.Join(t.TempDir(), "new rival")
	if err := os.WriteFile(binary, []byte("#!/bin/sh\nprintf '%s\\n' \"$@\"\nprintf 'new embedded skills\\n'\n"), 0700); err != nil {
		t.Fatal(err)
	}
	var out bytes.Buffer
	cmd := &cobra.Command{}
	cmd.SetContext(context.Background())
	cmd.SetOut(&out)
	if err := installUpdatedSkills(cmd, binary); err != nil {
		t.Fatal(err)
	}
	if want := "install\n--force\n--target\nauto\nnew embedded skills\n"; out.String() != want {
		t.Fatalf("got %q, want %q", out.String(), want)
	}
	if err := os.WriteFile(binary, []byte("#!/bin/sh\nexit 7\n"), 0700); err != nil {
		t.Fatal(err)
	}
	if err := installUpdatedSkills(cmd, binary); err == nil {
		t.Fatal("failed skill installation reported success")
	}
}

func TestUpdateUsesHomebrewBinaryNotOldEmbeddedSkills(t *testing.T) {
	bin, prefix := t.TempDir(), t.TempDir()
	t.Setenv("PATH", bin)
	t.Setenv("RIVAL_TEST_BREW_PREFIX", prefix)
	brew := "#!/bin/sh\ncase \"$1\" in\nupgrade) exit 0;;\n--prefix) printf '%s\\n' \"$RIVAL_TEST_BREW_PREFIX\";;\n*) exit 9;;\nesac\n"
	if err := os.WriteFile(filepath.Join(bin, "brew"), []byte(brew), 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.Mkdir(filepath.Join(prefix, "bin"), 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(prefix, "bin", "rival"), []byte("#!/bin/sh\nprintf 'new release skills\\n'\n"), 0700); err != nil {
		t.Fatal(err)
	}
	var out bytes.Buffer
	cmd := &cobra.Command{}
	cmd.SetContext(context.Background())
	cmd.SetOut(&out)
	if err := updateToVersion(cmd, "old", "new"); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "new release skills") {
		t.Fatalf("did not run new binary: %s", out.String())
	}
}

func TestCurrentVersionRefreshesSkillsForNewCodexInstall(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	if err := os.Mkdir(filepath.Join(home, ".codex"), 0700); err != nil {
		t.Fatal(err)
	}
	cmd := &cobra.Command{}
	cmd.SetOut(&bytes.Buffer{})
	if err := updateToVersion(cmd, "current", "current"); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(home, ".agents", "skills", "rival-claude", "SKILL.md")); err != nil {
		t.Fatal(err)
	}
}
