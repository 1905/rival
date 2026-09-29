package cmd

import (
	"fmt"

	"github.com/1905/rival/internal/config"
	"github.com/1905/rival/internal/session"
	"github.com/spf13/cobra"
)

var sessionsCmd = &cobra.Command{
	Use:   "sessions",
	Short: "List sessions",
	RunE:  sessionsAction,
}

func init() {
	sessionsCmd.Flags().Bool("active", false, "show only running sessions")
	sessionsCmd.Flags().Int("recent", 0, "show N most recent sessions")
	rootCmd.AddCommand(sessionsCmd)
}

func sessionsAction(cmd *cobra.Command, args []string) error {
	active, _ := cmd.Flags().GetBool("active")
	recent, _ := cmd.Flags().GetInt("recent")

	all := session.LoadAll()

	var sessions []*session.Session
	for _, s := range all {
		if active && s.Status != "running" {
			continue
		}
		sessions = append(sessions, s)
	}

	if recent > 0 && recent < len(sessions) {
		sessions = sessions[:recent]
	}

	if len(sessions) == 0 {
		fmt.Println("No sessions found.")
		return nil
	}

	for _, s := range sessions {
		status := s.Status
		dur := s.Duration
		if dur == "" && s.Status == "running" {
			dur = "running..."
		}
		id := s.ID
		if len(id) > 8 {
			id = id[:8]
		}
		fmt.Printf("%-8s  %-20s  %-10s  %-6s  %s\n",
			id, config.EngineLabel(s.CLI, s.Model), status, s.Effort, dur)
	}

	return nil
}
