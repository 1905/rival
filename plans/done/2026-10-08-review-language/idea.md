# Review language pass

**Date:** 2026-10-08
**Status:** done

The user wants every review result in controlled technical English. Reviewers get the rules in their prompt only, with no dictionary. After a code, security or plan review, rival checks the whole review text once with a Rust port of the antislop-eng checker and its full dictionary. When the check finds something, rival makes one repair call with the same model and harness, at low effort. There is no loop. The user sees only the repaired review.

The feature is internal: no command, no skill, no config switch. It starts after the rust-only spec (`plans/2026-10-08-rust-only/`) lands.
