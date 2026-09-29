package update

import (
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"time"
)

const (
	releasesURL = "https://api.github.com/repos/1905/rival/releases/latest"
	cacheTTL    = 24 * time.Hour
	httpTimeout = 2 * time.Second
)

type cache struct {
	Latest    string    `json:"latest"`
	CheckedAt time.Time `json:"checked_at"`
}

// Check queries GitHub for the latest release and prints a notice if an update is available.
// Designed to run in a goroutine — all errors are silently ignored.
func Check(currentVersion string) {
	if skipCheck() {
		return
	}

	cacheFile := cacheFilePath()
	if c, err := loadCache(cacheFile); err == nil && time.Since(c.CheckedAt) < cacheTTL {
		printIfNewer(currentVersion, c.Latest)
		return
	}

	latest, err := FetchLatest()
	if err != nil {
		return
	}

	_ = saveCache(cacheFile, latest)
	printIfNewer(currentVersion, latest)
}

func skipCheck() bool {
	for _, key := range []string{"CI", "RIVAL_NO_UPDATE_CHECK"} {
		if v := os.Getenv(key); v != "" && v != "0" && v != "false" {
			return true
		}
	}
	return false
}

func cacheFilePath() string {
	home, _ := os.UserHomeDir()
	return filepath.Join(home, ".rival", ".update-check")
}

func loadCache(path string) (*cache, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	var c cache
	if err := json.Unmarshal(data, &c); err != nil {
		return nil, err
	}
	return &c, nil
}

func saveCache(path, latest string) error {
	_ = os.MkdirAll(filepath.Dir(path), 0700)
	data, _ := json.Marshal(cache{Latest: latest, CheckedAt: time.Now()})
	return os.WriteFile(path, data, 0600)
}

// FetchLatest queries GitHub for the latest release tag name (without "v" prefix).
func FetchLatest() (string, error) {
	client := &http.Client{Timeout: httpTimeout}
	resp, err := client.Get(releasesURL)
	if err != nil {
		return "", err
	}
	defer func() { _ = resp.Body.Close() }()

	if resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("status %d", resp.StatusCode)
	}

	var release struct {
		TagName string `json:"tag_name"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&release); err != nil {
		return "", err
	}
	return strings.TrimPrefix(release.TagName, "v"), nil
}

func printIfNewer(current, latest string) {
	if latest == "" {
		return
	}
	cv := normalizeVersion(current)
	lv := normalizeVersion(latest)
	if lv != "" && cv != "" && lv != cv && lv > cv {
		currentDisplay := strings.TrimPrefix(current, "v")
		_, _ = fmt.Fprintf(os.Stderr, "\n  Update available: v%s → v%s — run 'rival update'\n\n", currentDisplay, latest)
	}
}

func normalizeVersion(v string) string {
	v = strings.TrimPrefix(v, "v")
	// Pad segments to 3-digit for string comparison: "3.5.0" → "003.005.000"
	parts := strings.Split(v, ".")
	if len(parts) != 3 {
		return v
	}
	return fmt.Sprintf("%03s.%03s.%03s", parts[0], parts[1], parts[2])
}
