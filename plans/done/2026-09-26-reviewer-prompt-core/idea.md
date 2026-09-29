# Reviewer prompt core — idea

**Date:** 2026-09-26
**Status:** done

User (2026-09-26), after a prompt review: "lets review prompting … is prompting good in your opinion?" → picked option A:
one shared reviewer core.

Scores from that review:
- security 8/10
- antislop 8/10
- judge 7/10
- plan 6/10
- megareview bug-hunter 6/10
- single-model review 4/10

Single-model review is what `/rival-codex review` and `/rival-claude review` run. Its problems:
- free-form prose, no JSON;
- it asks for speculative categories ("missing abstractions", "missing indexes") and then says "no speculation";
- no failure scenario required;
- no LOW severity, no confidence score.

Also observed: command mode prints the whole transcript. Today's Codex run was 495 KB, with the final answer printed twice.

First-guess scope:
- One prompt builder for every code review surface.
- A `failure_scenario` on every finding.
- One severity rubric shared by reviewers and the judge.
- Plan review verifies its claims against the repo.
- Drop the "ruthless senior staff engineer" persona.
- Command mode prints the formatted review, not the transcript.

Scope change (2026-09-26, same day): "antislop should use astra and opus5.5 too. no legacy models" →
"no megareview anymore so judge not needed too". The user picked:
- remove Sol entirely;
- delete megareview, the judge, `rival review` and `/rival-review`;
- one spec, removal first.
