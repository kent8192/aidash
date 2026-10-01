# Issue #44 local implementation verification

Recorded on 2026-10-01, macOS arm64, in the dedicated
`feat/issue-44-web-search` worktree. The implementation was uncommitted when this
local evidence was recorded, based on
`3cb803544c37ebbeb413bc8554accb0a06d916fa`. This record is separate from hosted CI
and release/deployment results. [verification.json](verification.json) binds the
code files and retained artifacts to their SHA-256 digests.

| Verification                                | Observed result                                                                                                                                                                                                                                                                                                                                                        |
| ------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Rust library                                | 160 passed. Includes Brave contract, provider-response bounds, language/domain mapping, credential admission and local HTTP adapter fixtures.                                                                                                                                                                                                                          |
| Web PostgreSQL/Harness cases                | 13 passed. Approval/denial and exact resumption, current quota/domain restrictions, private destinations/new input, foreign snapshots/cursors, Unicode/JSON limits, immutable citation/cache/revocation, recorded response replay, recovered redirect/long Retry-After, uncertain dispatch, Thread URL intent/idempotency/root revocation and one citation correction. |
| Related authorization/migration regressions | 84 passed: channel Threads 6, execution authorization 9, inference cancellation 7, interaction authorization 3, migrations 2, record constraints 29, Registry/execution contracts 16, resource authorization 12.                                                                                                                                                       |
| Browser                                     | 47 existing collaboration/working-area cases passed; all 3 Web cases passed after fixing the retained-Thread test's locator. Covers typed approval/stop/start, escaped PDF citation/revocation and retained research after catalog removal.                                                                                                                            |
| Controller                                  | 9 passed, including fixed Web operation contract and resource ceilings.                                                                                                                                                                                                                                                                                                |
| Real isolated format fixtures               | 9 passed through the admitted gVisor reader: HTML/script inertness, Unicode/gzip text, two-page text PDF, scanned, encrypted, malformed, over-200-page PDF, decompression limit and text limit.                                                                                                                                                                        |
| Real public HTTP/Harness/citation path      | Passed using `https://www.rfc-editor.org/rfc/rfc2606.txt`: exact disclosure, actual fetch, isolated extraction, local find, final answer and authenticated immutable excerpt API. One page attempt; zero search attempts.                                                                                                                                              |
| Build/static checks                         | Dashboard/API generation/build, Rustfmt, Clippy with all targets/features and warnings denied, ESLint and Trunk's Ruff/Prettier checks.                                                                                                                                                                                                                                |
| Pilot rehearsal                             | Frozen 20 queries (10 JA, 10 EN), inclusive September 2–October 1 range for pair 10, no placeholder dates and zero provider calls. Exclusive creation preserves an existing manifest.                                                                                                                                                                                  |

The real reader used the pinned image
`docker.io/library/aidash-sandbox@sha256:bc0e43b54e97c615ad76f96dd8ad4483862d3ad796fd48e395679280347f4a2a`.
Its nine recorded fixture durations have a 4.058-second median and 4.386-second
maximum, including controller staging and collection. These are controlled
fixture observations, not provider latency or production percentiles. Every
parsed fixture reported verified child-process prohibition; fixture input and
copied stdout were removed after acknowledgement. The owned disposable
`aidash-web44-20261001` cluster/controller were removed after verification.

Two test setup issues were corrected and rechecked: the initial broader Peer
test used a synthetic credential shorter than the repository's required length;
the corrected standard fixture passed all 16 Registry/execution cases. The added
retained-history browser test initially looked for “Reply in thread” after a
Thread already existed; the observed button was “Open thread”. All three focused
Web browser cases passed with the corrected locator. No product fix was inferred
from either test setup failure.

The parser's initial real-runtime failure did require a product fix:
`RLIMIT_NPROC=0` on the gVisor init task also prevented trusted collector execs.
The fixed parser applies an architecture-bound seccomp child-creation filter to
itself and runs a real fork-denial probe before consuming source bytes. The
trusted collector keeps its admitted process budget. The nine real format
fixtures and public Harness reader were rerun successfully on that image.

No Brave account or key was configured. Paid provider requests, measured ranking,
all six current-information answers, billed-cost reconciliation and the
[required live 20-query pilot](../../../design/2026-09-25-web-search-evaluation.md)
remain **NOT RUN**. Search and production admission remain disabled by default;
Issue #44 remains open for account eligibility and that rollout evidence.

See [operator instructions](../../web-research.md) for configuration, retention,
rollback and reproduction. Published evidence contains no token, kubeconfig,
private profile, original downloaded document or browser trace with credentials.

The [research controls screenshot](web-research-ui.png) and
[citation excerpt screenshot](citation-ui.png) use the browser tests' synthetic
API fixtures. They show the rendered UI, including an inert script-shaped
excerpt; they are not live Brave account, request or billing evidence.
