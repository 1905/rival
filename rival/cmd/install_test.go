package cmd

import (
	"bufio"
	"bytes"
	"crypto/sha256"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/1905/rival/internal/skills"
)

func TestSkillTargets(t *testing.T) {
	for _, tt := range []struct {
		target string
		codex  bool
		want   []string
	}{
		{"auto", false, []string{"claude"}},
		{"auto", true, []string{"claude", "codex"}},
		{"claude", true, []string{"claude"}},
		{"codex", false, []string{"codex"}},
		{"all", false, []string{"claude", "codex"}},
	} {
		t.Run(fmt.Sprintf("%s/%t", tt.target, tt.codex), func(t *testing.T) {
			home := t.TempDir()
			targets, err := skillTargets(home, tt.target, tt.codex)
			if err != nil {
				t.Fatal(err)
			}
			var hosts []string
			for _, target := range targets {
				hosts = append(hosts, target.host)
				folder := ".claude"
				if target.host == "codex" {
					folder = ".agents"
				}
				if target.base != filepath.Join(home, folder, "skills") {
					t.Fatalf("wrong destination: %s", target.base)
				}
			}
			if !reflect.DeepEqual(hosts, tt.want) {
				t.Fatalf("got %v, want %v", hosts, tt.want)
			}
		})
	}
	if _, err := skillTargets(t.TempDir(), "typo", true); err == nil {
		t.Fatal("invalid target accepted")
	}
}

func TestDetectCodex(t *testing.T) {
	for _, signal := range []string{"absent", "cli", "config", "custom-home", "user-app", "system-app"} {
		t.Run(signal, func(t *testing.T) {
			home, bin, apps := t.TempDir(), t.TempDir(), t.TempDir()
			t.Setenv("PATH", bin)
			t.Setenv("CODEX_HOME", "")
			var dir string
			switch signal {
			case "cli":
				if err := os.WriteFile(filepath.Join(bin, "codex"), []byte("#!/bin/sh\nexit 0\n"), 0700); err != nil {
					t.Fatal(err)
				}
			case "config":
				dir = filepath.Join(home, ".codex")
			case "custom-home":
				dir = filepath.Join(home, "custom-codex")
				t.Setenv("CODEX_HOME", dir)
			case "user-app":
				dir = filepath.Join(home, "Applications", "Codex.app")
			case "system-app":
				dir = filepath.Join(apps, "Codex.app")
			}
			if dir != "" {
				if err := os.MkdirAll(dir, 0700); err != nil {
					t.Fatal(err)
				}
			}
			if got := detectCodex(home, apps); got != (signal != "absent") {
				t.Fatalf("detection for %s = %t", signal, got)
			}
		})
	}
}

func TestInstallBothHostsAndPreserveUnrelatedSkills(t *testing.T) {
	targets, err := skillTargets(t.TempDir(), "all", false)
	if err != nil {
		t.Fatal(err)
	}
	for _, target := range targets {
		unrelated := filepath.Join(target.base, "rival-personal", "SKILL.md")
		if err := writeSkill(filepath.Dir(unrelated), unrelated, []byte("my skill")); err != nil {
			t.Fatal(err)
		}
		var out bytes.Buffer
		if err := installSkills(target, false, bufio.NewReader(strings.NewReader("")), &out); err != nil {
			t.Fatal(err)
		}
		for _, name := range skills.Names {
			got, err := os.ReadFile(filepath.Join(target.base, name, "SKILL.md"))
			if err != nil {
				t.Fatal(err)
			}
			want, version, err := readEmbeddedSkill(name)
			if err != nil {
				t.Fatal(err)
			}
			if target.host == "codex" {
				want, err = skills.CodexSkill(name, version)
			}
			if err != nil {
				t.Fatal(err)
			}
			if !bytes.Equal(got, want) || parseVersion(string(got)) != version {
				t.Fatalf("incorrect %s skill %s", target.host, name)
			}
		}
		if data, err := os.ReadFile(unrelated); err != nil || string(data) != "my skill" {
			t.Fatalf("unrelated skill changed: %q %v", data, err)
		}
	}
}

func TestInstallOverwriteAndBufferedAnswers(t *testing.T) {
	target := skillTarget{"codex", t.TempDir()}
	run := func(force bool, answers string) {
		t.Helper()
		if err := installSkills(target, force, bufio.NewReader(strings.NewReader(answers)), &bytes.Buffer{}); err != nil {
			t.Fatal(err)
		}
	}
	run(false, "")
	paths := []string{filepath.Join(target.base, skills.Names[0], "SKILL.md"), filepath.Join(target.base, skills.Names[1], "SKILL.md")}
	old := []byte("---\nversion: old\n---\ncustom")
	for _, path := range paths {
		if err := os.WriteFile(path, old, 0600); err != nil {
			t.Fatal(err)
		}
	}
	run(false, "n\nn\n")
	for _, path := range paths {
		data, _ := os.ReadFile(path)
		if !bytes.Equal(data, old) {
			t.Fatal("declined update overwrote skill")
		}
	}
	run(false, "y\ny\n")
	for _, path := range paths {
		data, _ := os.ReadFile(path)
		if bytes.Equal(data, old) {
			t.Fatal("buffered confirmation was lost")
		}
	}
	data, err := os.ReadFile(paths[0])
	if err != nil {
		t.Fatal(err)
	}
	custom := append(data, []byte("\nlocal customization")...)
	if err := os.WriteFile(paths[0], custom, 0600); err != nil {
		t.Fatal(err)
	}
	run(false, "")
	got, _ := os.ReadFile(paths[0])
	if !bytes.Equal(got, custom) {
		t.Fatal("same-version customization overwritten")
	}
	run(true, "")
	got, _ = os.ReadFile(paths[0])
	if !bytes.Equal(got, data) {
		t.Fatal("force did not refresh skill")
	}
}

func TestRemoveSkillDirsByHashRemovesOnlyExactMatches(t *testing.T) {
	base := t.TempDir()
	retired := "rival-retired-fixture"
	kept := "rival-current-fixture"
	for _, name := range []string{retired, kept} {
		if err := os.Mkdir(filepath.Join(base, name), 0o755); err != nil {
			t.Fatal(err)
		}
	}

	sum := fmt.Sprintf("%x", sha256.Sum256([]byte(retired)))
	removed, err := removeSkillDirsByHash(base, map[string]struct{}{sum: {}})
	if err != nil {
		t.Fatal(err)
	}
	if removed != 1 {
		t.Fatalf("removed = %d, want 1", removed)
	}
	if _, err := os.Stat(filepath.Join(base, retired)); !os.IsNotExist(err) {
		t.Fatalf("retired directory still exists: %v", err)
	}
	if _, err := os.Stat(filepath.Join(base, kept)); err != nil {
		t.Fatalf("current directory was removed: %v", err)
	}
}

func TestRetiredSkillCleanupHashesStayConfigured(t *testing.T) {
	if len(retiredSkillNameHashes) != 2 {
		t.Fatalf("retired skill cleanup hash count = %d, want 2", len(retiredSkillNameHashes))
	}
}

// An installed rival-review (megareview, removed 2026-09-26) is cleaned up by
// install on both hosts.
func TestInstallRemovesRetiredReviewSkill(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	targets, err := skillTargets(home, "all", false)
	if err != nil {
		t.Fatal(err)
	}
	for _, target := range targets {
		stale := filepath.Join(target.base, "rival-review", "SKILL.md")
		if err := writeSkill(filepath.Dir(stale), stale, []byte("---\nname: rival-review\n---\n")); err != nil {
			t.Fatal(err)
		}
		var out bytes.Buffer
		if err := installSkills(target, false, bufio.NewReader(strings.NewReader("")), &out); err != nil {
			t.Fatal(err)
		}
		if _, err := os.Stat(filepath.Dir(stale)); !os.IsNotExist(err) {
			t.Fatalf("%s: rival-review was not removed by install (stat err %v)", target.host, err)
		}
	}
}
