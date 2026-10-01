//go:build !windows

package executor

import (
	"errors"
	"os"
	"os/exec"
	"syscall"
)

// setProcessGroup starts the provider in its own process group and makes
// context cancellation SIGKILL that whole group. Provider launchers (the npm
// `codex` wrapper) spawn the native binary with inherited stdio and cannot
// forward SIGKILL; killing only the direct child would leave the grandchild
// holding the stdout/stderr pipes open forever.
func setProcessGroup(cmd *exec.Cmd) {
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	cmd.Cancel = func() error {
		// With Setpgid the group ID equals the leader's PID.
		err := syscall.Kill(-cmd.Process.Pid, syscall.SIGKILL)
		if errors.Is(err, syscall.ESRCH) {
			return os.ErrProcessDone
		}
		return err
	}
}
