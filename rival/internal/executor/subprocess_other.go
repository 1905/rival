//go:build windows

package executor

import "os/exec"

// setProcessGroup is a no-op on Windows: syscall.SysProcAttr has no Setpgid
// field. rival targets darwin+linux for releases; this keeps the package
// compilable for GOOS=windows tooling/CI. exec.CommandContext's default
// Cancel (kill the direct child) and cmd.WaitDelay still apply.
func setProcessGroup(*exec.Cmd) {}
