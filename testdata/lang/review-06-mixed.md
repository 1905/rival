# Several issues in the update flow

1. The updater has been checking the latest release on every start, which leverages the GitHub API unnecessarily and might hit the rate limit.
2. He who reads the cache file assumes it is always valid JSON; if it isn't, the whole command fails instead of falling back gracefully.
3. The download is not verified against `checksums.txt`, etc.

Overall, these are easy wins: cache the check for 24 hours, treat a broken cache as missing, and verify the checksum before installing anything. We'd recommend doing all three in one change.

```rust
let cache = read_cache(&path).unwrap_or_default(); // keep this as is
```
