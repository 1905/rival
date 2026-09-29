package cmd

import (
	"bufio"
	"crypto/sha256"
	"fmt"
	"io"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/1905/rival/internal/skills"
	"github.com/spf13/cobra"
)

var forceInstall bool
var installTarget string

// retiredSkillNameHashes lets upgrades remove two retired integration skills
// without retaining their obsolete public names anywhere in the shipped tree.
// The values are SHA-256(name), not content hashes.
var retiredSkillNameHashes = map[string]struct{}{
	"206a1c0a9997719ba41cc76d4e2e2699ff4d2000fd94fd1bad99ba5d73ddc98a": {},
	"75160929d947197a4444be684d0c9a67784cc4ebd84b45cd1de2234a6981056a": {},
}

var installCmd = &cobra.Command{
	Use:   "install",
	Short: "Install skills for Claude Code and Codex",
	Args:  cobra.NoArgs,
	RunE:  runInstall,
}

func init() {
	installCmd.Flags().BoolVar(&forceInstall, "force", false, "overwrite without prompting")
	installCmd.Flags().StringVar(&installTarget, "target", "auto", "skill host: auto, claude, codex, all")
	rootCmd.AddCommand(installCmd)
}

func runInstall(cmd *cobra.Command, args []string) error {
	home, err := os.UserHomeDir()
	if err != nil {
		return fmt.Errorf("get home dir: %w", err)
	}
	targets, err := skillTargets(home, installTarget, codexInstalled(home))
	if err != nil {
		return err
	}
	if installTarget == "auto" && len(targets) == 1 {
		cmd.Println("Codex not detected; skipped (use --target codex to install explicitly).")
	}
	return installTargets(cmd, targets, forceInstall)
}

type skillTarget struct{ host, base string }

func skillTargets(home, target string, hasCodex bool) ([]skillTarget, error) {
	claude := skillTarget{"claude", filepath.Join(home, ".claude", "skills")}
	codex := skillTarget{"codex", filepath.Join(home, ".agents", "skills")}
	switch target {
	case "auto":
		if hasCodex {
			return []skillTarget{claude, codex}, nil
		}
		return []skillTarget{claude}, nil
	case "claude":
		return []skillTarget{claude}, nil
	case "codex":
		return []skillTarget{codex}, nil
	case "all":
		return []skillTarget{claude, codex}, nil
	default:
		return nil, fmt.Errorf("unknown install target %q; use auto, claude, codex, or all", target)
	}
}

func codexInstalled(home string) bool {
	return detectCodex(home, "/Applications")
}

func detectCodex(home, applications string) bool {
	if _, err := exec.LookPath("codex"); err == nil {
		return true
	}
	for _, path := range []string{os.Getenv("CODEX_HOME"), filepath.Join(home, ".codex"), filepath.Join(home, "Applications", "Codex.app"), filepath.Join(applications, "Codex.app")} {
		if info, err := os.Stat(path); err == nil && info.IsDir() {
			return true
		}
	}
	return false
}

func installTargets(cmd *cobra.Command, targets []skillTarget, force bool) error {
	reader := bufio.NewReader(cmd.InOrStdin())
	for _, target := range targets {
		if err := installSkills(target, force, reader, cmd.OutOrStdout()); err != nil {
			return err
		}
	}
	return nil
}

func installSkills(target skillTarget, force bool, reader *bufio.Reader, out io.Writer) error {
	targetBase := target.base
	_, _ = fmt.Fprintf(out, "Installing %s skills to %s\n\n", target.host, targetBase)
	var installed, updated, skipped int

	for _, name := range skills.Names {
		srcContent, srcVersion, err := readEmbeddedSkill(name)
		if err != nil {
			return fmt.Errorf("read embedded skill %s: %w", name, err)
		}
		if target.host == "codex" {
			srcContent, err = skills.CodexSkill(name, srcVersion)
			if err != nil {
				return err
			}
		}

		targetDir := filepath.Join(targetBase, name)
		targetFile := filepath.Join(targetDir, "SKILL.md")

		if _, err := os.Stat(targetFile); os.IsNotExist(err) {
			// New install
			if err := writeSkill(targetDir, targetFile, srcContent); err != nil {
				return err
			}
			_, _ = fmt.Fprintf(out, "  ✓ %s — installed (v%s)\n", name, srcVersion)
			installed++
			continue
		}

		// Existing — compare versions
		existingContent, err := os.ReadFile(targetFile)
		if err != nil {
			return fmt.Errorf("read %s: %w", targetFile, err)
		}
		dstVersion := parseVersion(string(existingContent))

		if srcVersion == dstVersion && !force {
			_, _ = fmt.Fprintf(out, "  · %s — already up to date (v%s)\n", name, srcVersion)
			skipped++
			continue
		}

		if !force {
			_, _ = fmt.Fprintf(out, "  ? %s — update v%s → v%s? [y/N] ", name, dstVersion, srcVersion)
			answer, _ := reader.ReadString('\n')
			answer = strings.TrimSpace(strings.ToLower(answer))
			if answer != "y" && answer != "yes" {
				_, _ = fmt.Fprintf(out, "    skipped\n")
				skipped++
				continue
			}
		}

		if err := writeSkill(targetDir, targetFile, srcContent); err != nil {
			return err
		}
		_, _ = fmt.Fprintf(out, "  ✓ %s — updated (v%s → v%s)\n", name, dstVersion, srcVersion)
		updated++
	}

	// Clean up deprecated skills.
	var removed int
	for _, name := range skills.Deprecated {
		targetDir := filepath.Join(targetBase, name)
		if _, err := os.Stat(targetDir); err == nil {
			if err := os.RemoveAll(targetDir); err != nil {
				_, _ = fmt.Fprintln(out, "  ✗ deprecated skill cleanup failed — check permissions in the skills directory")
			} else {
				_, _ = fmt.Fprintln(out, "  🗑 deprecated skill removed")
				removed++
			}
		}
	}
	hashRemoved, err := removeSkillDirsByHash(targetBase, retiredSkillNameHashes)
	if err != nil {
		_, _ = fmt.Fprintln(out, "  ✗ retired skill cleanup failed — check permissions in the skills directory")
	} else if hashRemoved > 0 {
		_, _ = fmt.Fprintf(out, "  🗑 %d retired skill(s) removed\n", hashRemoved)
		removed += hashRemoved
	}

	_, _ = fmt.Fprintln(out)
	_, _ = fmt.Fprintf(out, "Done: %d installed, %d updated, %d up to date, %d removed\n", installed, updated, skipped, removed)
	return nil
}

func removeSkillDirsByHash(targetBase string, hashes map[string]struct{}) (int, error) {
	entries, err := os.ReadDir(targetBase)
	if os.IsNotExist(err) {
		return 0, nil
	}
	if err != nil {
		return 0, err
	}

	removed := 0
	for _, entry := range entries {
		sum := fmt.Sprintf("%x", sha256.Sum256([]byte(entry.Name())))
		if _, ok := hashes[sum]; !ok {
			continue
		}
		if err := os.RemoveAll(filepath.Join(targetBase, entry.Name())); err != nil {
			return removed, err
		}
		removed++
	}
	return removed, nil
}

func readEmbeddedSkill(name string) ([]byte, string, error) {
	content, err := fs.ReadFile(skills.Files, filepath.Join(name, "SKILL.md"))
	if err != nil {
		return nil, "", err
	}
	return content, parseVersion(string(content)), nil
}

func writeSkill(dir, file string, content []byte) error {
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return fmt.Errorf("mkdir %s: %w", dir, err)
	}
	if err := os.WriteFile(file, content, 0o644); err != nil {
		return fmt.Errorf("write %s: %w", file, err)
	}
	return nil
}

// parseVersion extracts the version: field from YAML frontmatter.
// Frontmatter is between the first and second "---" lines.
func parseVersion(content string) string {
	lines := strings.Split(content, "\n")
	inFrontmatter := false
	for _, line := range lines {
		trimmed := strings.TrimSpace(line)
		if trimmed == "---" {
			if !inFrontmatter {
				inFrontmatter = true
				continue
			}
			break // end of frontmatter
		}
		if inFrontmatter && strings.HasPrefix(trimmed, "version:") {
			return strings.TrimSpace(strings.TrimPrefix(trimmed, "version:"))
		}
	}
	return "unknown"
}
