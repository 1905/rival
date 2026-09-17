package gitscope

import "strings"

// RepositoryEnv clears Git's repository-local overrides before changing workdir.
// See https://git-scm.com/docs/githooks and git rev-parse --local-env-vars.
// Keep host configuration and SSH authentication available for fetch.
func RepositoryEnv(env []string) []string {
	clean := make([]string, 0, len(env))
	for _, item := range env {
		key, _, _ := strings.Cut(item, "=")
		switch key {
		case "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_CONFIG", "GIT_CONFIG_PARAMETERS",
			"GIT_CONFIG_COUNT", "GIT_OBJECT_DIRECTORY", "GIT_DIR", "GIT_WORK_TREE",
			"GIT_IMPLICIT_WORK_TREE", "GIT_GRAFT_FILE", "GIT_INDEX_FILE",
			"GIT_NO_REPLACE_OBJECTS", "GIT_REPLACE_REF_BASE", "GIT_PREFIX",
			"GIT_SHALLOW_FILE", "GIT_COMMON_DIR":
			continue
		}
		clean = append(clean, item)
	}
	return clean
}
