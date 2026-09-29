package cmd

import (
	"fmt"
	"time"

	"github.com/1905/rival/internal/queue"
	"github.com/spf13/cobra"
)

var queueCmd = &cobra.Command{
	Use:   "queue",
	Short: "Inspect the review queue",
	RunE:  queueListAction,
}

var queueClearCmd = &cobra.Command{
	Use:   "clear",
	Short: "Remove dead queue tickets (--force removes all)",
	RunE:  queueClearAction,
}

func init() {
	queueClearCmd.Flags().Bool("force", false, "remove ALL tickets, not just dead ones")
	queueCmd.AddCommand(queueClearCmd)
	rootCmd.AddCommand(queueCmd)
}

func queueListAction(cmd *cobra.Command, args []string) error {
	entries, err := queue.New().List()
	if err != nil {
		return fmt.Errorf("read queue: %w", err)
	}
	if len(entries) == 0 {
		fmt.Println("Queue is empty.")
		return nil
	}

	now := time.Now()
	fmt.Printf("%-4s  %-8s  %-12s  %-7s  %-9s  %s\n", "POS", "STATE", "MODE", "PID", "WAIT", "WORKDIR")
	for _, e := range entries {
		t := e.Ticket
		pos := "-"
		state := t.State
		if e.Position > 0 {
			pos = fmt.Sprintf("#%d", e.Position)
		}
		// Wait time: running tickets show time since promotion, waiting since creation.
		since := t.CreatedAt
		if t.State == queue.StateRunning && t.StartedAt != nil {
			since = *t.StartedAt
		}
		wait := now.Sub(since).Round(time.Second).String()
		// Flag a waiting ticket sitting far past the default timeout as suspect.
		if t.State == queue.StateWaiting && now.Sub(t.CreatedAt) > 30*time.Minute {
			state = "stale?"
		}
		fmt.Printf("%-4s  %-8s  %-12s  %-7d  %-9s  %s\n", pos, state, t.Mode, t.PID, wait, t.WorkDir)
	}
	return nil
}

func queueClearAction(cmd *cobra.Command, args []string) error {
	force, _ := cmd.Flags().GetBool("force")
	removed, err := queue.New().Clear(force)
	if err != nil {
		return fmt.Errorf("clear queue: %w", err)
	}
	noun := "dead ticket"
	if force {
		noun = "ticket"
	}
	if removed == 1 {
		fmt.Printf("Removed 1 %s.\n", noun)
	} else {
		fmt.Printf("Removed %d %ss.\n", removed, noun)
	}
	return nil
}
