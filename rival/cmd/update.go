package cmd

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/1905/rival/internal/update"
	"github.com/spf13/cobra"
)

var updateCmd = &cobra.Command{
	Use:   "update",
	Short: "Update rival to the latest version via Homebrew",
	RunE:  runUpdate,
}

func init() {
	rootCmd.AddCommand(updateCmd)
}

func runUpdate(cmd *cobra.Command, args []string) error {
	// Check latest release from GitHub
	fmt.Print("Checking for updates... ")
	latest, err := update.FetchLatest()
	if err != nil {
		return fmt.Errorf("check latest version: %w", err)
	}
	return updateToVersion(cmd, Version, latest)
}

func updateToVersion(cmd *cobra.Command, current, latest string) error {
	if latest == current {
		fmt.Printf("already on latest (v%s)\n", current)
		home, err := os.UserHomeDir()
		if err != nil {
			return err
		}
		targets, err := skillTargets(home, "auto", codexInstalled(home))
		if err != nil {
			return err
		}
		return installTargets(cmd, targets, true)
	}

	fmt.Printf("v%s → v%s\n\n", current, latest)

	// Upgrade via brew
	fmt.Println("Upgrading via Homebrew...")
	brew := exec.CommandContext(cmd.Context(), "brew", "upgrade", "1905/tap/rival")
	brew.Stdout = cmd.OutOrStdout()
	brew.Stderr = cmd.ErrOrStderr()
	if err := brew.Run(); err != nil {
		// If brew upgrade fails (e.g. already latest), try reinstall
		fmt.Println("brew upgrade failed, trying reinstall...")
		reinstall := exec.CommandContext(cmd.Context(), "brew", "reinstall", "1905/tap/rival")
		reinstall.Stdout = cmd.OutOrStdout()
		reinstall.Stderr = cmd.ErrOrStderr()
		if err := reinstall.Run(); err != nil {
			return fmt.Errorf("brew reinstall: %w", err)
		}
	}

	// Reinstall skills
	fmt.Println("\nUpdating skills...")
	prefixCmd := exec.CommandContext(cmd.Context(), "brew", "--prefix", "rival")
	prefix, err := prefixCmd.Output()
	if err != nil {
		return fmt.Errorf("locate upgraded rival: %w", err)
	}
	binary := filepath.Join(strings.TrimSpace(string(prefix)), "bin", "rival")
	if err := installUpdatedSkills(cmd, binary); err != nil {
		return fmt.Errorf("install skills: %w", err)
	}

	fmt.Printf("\n✓ Updated to v%s\n", latest)
	return nil
}

// Re-exec the Homebrew installation: this process still embeds the old skills.
func installUpdatedSkills(cmd *cobra.Command, binary string) error {
	install := exec.CommandContext(cmd.Context(), binary, "install", "--force", "--target", "auto")
	install.Stdin = cmd.InOrStdin()
	install.Stdout = cmd.OutOrStdout()
	install.Stderr = cmd.ErrOrStderr()
	return install.Run()
}
