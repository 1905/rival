# Timeout is ignored for the second call

The retry loop in `run_with_retry` doesn't pass the original deadline to the second attempt, so the overall run can exceed RIVAL_RUN_TIMEOUT by a significant margin.

Basically, the deadline is being recomputed from scratch each time, which is a subtle but robust-looking bug that has been present since the refactor.

Suggestion: Compute the deadline once, store it, and utilize it for every attempt; also consider adding a test that verifies the total elapsed time stays under the limit.
