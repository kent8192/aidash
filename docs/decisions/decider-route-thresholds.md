# Decider Route thresholds (2026-10-19)

Decided for #153 under ADR 0066: the `cloudflare/clef` Decisions Route keeps
content at a threshold of **0.2**. Jev's System One Route keeps 0.5.

## Method

`scripts/evaluate-decider-routes.py` sent the six synthetic sessions in
`scripts/decider_route_fixtures.py` (39 tool pairs) once to `typesafe/jev-1.14`
through the System One API and once to `cloudflare/clef` on each of its three
OpenRouter hosts through the Decisions API, pinned with `provider.only` and with
zero data retention. The states and questions are the ones the compaction
builder sends, with the two newest tool events protected. For each candidate
threshold the script compares Clef's keep, truncate or drop branch with Jev's at
0.5. The whole run cost USD 0.0071. The raw answers are in
`decider-route-evaluation-2026-10-19.json`.

## Results

Jev at 0.5 removes 0.86 of the tool content (a dropped pair counts
fully, a truncated result half). Each cell is agreement with Jev / share of
pairs where Clef removes more than Jev / share of tool content Clef removes.

| Threshold | Cloudflare         | DeepInfra          | PrimeIntellect     |
| --------- | ------------------ | ------------------ | ------------------ |
| 0.15      | 0.72 / 0.00 / 0.63 | 0.72 / 0.03 / 0.68 | 0.82 / 0.05 / 0.81 |
| 0.2       | 0.82 / 0.08 / 0.83 | 0.85 / 0.05 / 0.83 | 0.85 / 0.08 / 0.86 |
| 0.25      | 0.85 / 0.08 / 0.86 | 0.87 / 0.08 / 0.88 | 0.87 / 0.10 / 0.91 |
| 0.3       | 0.87 / 0.08 / 0.88 | 0.85 / 0.13 / 0.94 | 0.87 / 0.13 / 0.95 |
| 0.5       | 0.85 / 0.15 / 0.97 | 0.87 / 0.13 / 0.95 | 0.85 / 0.15 / 0.97 |

## Choice

Clef answers lower probabilities than Jev for the same content, so at Jev's 0.5
it removes more than Jev on every host. At 0.2 no host removes more tool content
than Jev overall, agreement stays at 0.82 to 0.85, and Clef removes more than Jev
on at most 8 percent of pairs. OpenRouter may send a request to any of the three
hosts, so the threshold must hold on the host that removes the most; 0.25 would
already remove more than Jev on DeepInfra and PrimeIntellect. A lower threshold
than 0.2 keeps more but agrees less with Jev and removes too little to fit
oversized contexts.

These are measurements on six synthetic sessions, not a quality guarantee.
Running the script again on a new model or host set gives the numbers for a new
Decider version; changing a Route's threshold makes a new version that needs a
new Catalog Approval.
