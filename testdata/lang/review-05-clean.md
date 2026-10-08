# Missing check on the return value

The function `write_ticket` ignores the result of `fsync`. If the disk is full, the ticket file can be empty after a crash.

Failure scenario: the disk is full, `fsync` fails, the process stops, and the next run reads an empty ticket.

Suggestion: Return the `fsync` error to the caller.
