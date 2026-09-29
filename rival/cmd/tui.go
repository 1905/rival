package cmd

import (
	"fmt"

	tea "charm.land/bubbletea/v2"
	"github.com/1905/rival/internal/dashboard"
	"github.com/spf13/cobra"
)

var tuiCmd = &cobra.Command{
	Use:   "tui",
	Short: "Launch the TUI dashboard",
	RunE: func(cmd *cobra.Command, args []string) error {
		dashboard.Version = Version
		m := dashboard.New()
		p := tea.NewProgram(m)
		if _, err := p.Run(); err != nil {
			return fmt.Errorf("tui: %w", err)
		}
		return nil
	},
}

func init() {
	rootCmd.AddCommand(tuiCmd)
}
