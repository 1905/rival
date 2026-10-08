# Data race in the session cache

The `SessionCache::insert` method in `src/cache.rs:42` is called from two threads without holding the lock, which could potentially lead to a corrupted map state.

It's important to note that this may result in lost updates; we should ensure that the mutex is held for the entire duration of the operation.

Failure scenario: two concurrent requests that update the same key will race, and the second write might overwrite the first one, e.g. when the TUI and the reaper run at once.

Suggestion: Take the lock before reading the map, and release it only after the write has been completed.
