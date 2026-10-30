---
status: accepted
---

# A compaction Decider may truncate kept results by rank until the request fits

Decided on 2026-10-30 for #611. The compaction rule `compaction.keep/1` reads each candidate on its own:
a keep-result probability at or above the Route's threshold keeps the candidate, and the threshold may not
exceed 0.5 (ADR 0066). In live Runs on 2026-10-29 (#122), Jev's keep probabilities clustered around that
threshold. Most fell between 0.40 and 0.55, so an archive page the Run no longer needed was often kept on a
0.51. When nothing else could be removed, the Run paused with `context_unreducible`. That was the main reason
recovery Runs did not finish. Jev's ranking still separated the history the Run needed from the rest.

We added a second compaction rule, `compaction.fit/1`, which a Decider version opts into by naming it. It
starts from the branches that `compaction.keep/1` chooses. While the projection does not fit, it truncates
the results of the still-kept, unprotected candidates one at a time. It goes in ascending keep-result
probability, with older history first on ties. It never truncates a result whose probability reaches 0.7.
If the projection still does not fit, the outcome stays `insufficient`, as before. Truncating keeps the call
and the head of the result, which is the same `TruncateResult` branch the threshold already uses. Nothing is
dropped beyond what the threshold chose.

## Considered Options

- **Lower the bar by raising the Route threshold.** A Route takes its conservative action at or above its
  threshold, so a higher threshold keeps more, not less. Above 0.5 the threshold would invert the meaning of
  a Route's answer. Rejected.
- **Drop the lowest-ranked candidates instead of truncating them.** Dropping also removes the call, so the
  model loses the record that it asked. Truncation frees most of the space, because the result body is the
  large part. Rejected for this rule. A later rule may drop.
- **Make rank order the default for every Decider.** It changes what every approved Decider version does to
  Run content after approval. The rule is part of the version and its contract digest, so a Tenant opts in by
  approving a version that names it. The built-in Decider keeps `compaction.keep/1` until a measurement
  justifies a new built-in version. Rejected for now.
- **Fall back to the Summary Stage when pruning keeps too much.** That is a separate decision (ADR 0010
  forbids it only when the Decider is unavailable). Not decided here.

## Consequences

- Evidence records the rule. Replay recomputes the `compaction.keep/1` branches, then checks that the
  truncated results are exactly the first ones the rank order names. Sizes are not replayed; the projection
  digest still commits to the result.
- Evidence of `compaction.keep/1` versions replays unchanged.
- The 0.7 ceiling is part of the rule. Changing it makes a new rule value.
- Jev never sees result bodies: the compaction state withholds them. Its ranking comes from the goal, the
  tool, the input and the result size, so the rank order is only as good as those signals.
- (2026-10-30, #626) A live A/B of the two rules showed the cost of truncating more. Under `compaction.fit/1`,
  no Run paused as unreducible, but models fetched truncated pages again: 165 re-reads against 62. Each
  re-read overflowed the window again and triggered another Decider call, until the Run hit its step or
  call limit. The likely cause was the truncation marker, which said that the full result remained in the execution
  journal. The marker now says the cut was deliberate, gives the head and the total length, and asks for the
  tool to be called again only if the rest is still needed verbatim. An elided result carries a note to the
  same effect. Both markers apply to every rule, because they are part of the projection and not of the
  decision.
  In the next A/B, `compaction.fit/1` with the new marker made 108 re-reads instead of 165, and it hit the
  step limit 4 times instead of 7. It kept the corrected constraints in 7 of 12 Runs, against 3 under
  `compaction.keep/1`. Re-reads are reduced, not gone (#626 stays open).
