package main

import (
	"os"

	"github.com/1905/rival/cmd"
	"github.com/1905/rival/internal/telemetry"
	"github.com/joho/godotenv"
	"github.com/rs/zerolog"
	"github.com/rs/zerolog/log"
)

// Set via ldflags at build time.
var version = "dev"

func main() {
	// Load .env silently (no error if missing).
	_ = godotenv.Load()

	// Zerolog to stderr, JSON output.
	log.Logger = zerolog.New(os.Stderr).With().
		Timestamp().
		Str("app", "rival").
		Logger()

	telemetry.Init(version)
	defer telemetry.Flush()

	cmd.Version = version
	cmd.Execute()
}
