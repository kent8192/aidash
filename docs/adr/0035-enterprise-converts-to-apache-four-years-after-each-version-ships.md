# Enterprise converts to Apache-2.0 four years after each version first ships

Decided on 2026-10-13 (#124). Enterprise is licensed under BUSL-1.1 with
`Additional Use Grant: None`, so only Aidash runs it in production. No
commercial license is offered. Cloud and Dedicated, both on infrastructure
Aidash controls, are the only production route.

Each Enterprise version gets one Change Date, four years after that version
first carries Enterprise code publicly. A version is every snapshot and release
of one `x.y.z`. The date is fixed at that first publication. Its Change License
is Apache-2.0, the license of Community.

## Considered Options

- **MPL-2.0 as the Change License** (the original draft of #124). Its file-level
  copyleft applies only on distribution, so it does not bind a competing
  service. It would also leave a third license in a tree whose Community is
  Apache-2.0. Rejected.
- **A Change Date per release tag or per commit.** Development is public on
  `develop/x.y.z`, so snapshots are published before any tag. Dating the
  release would postpone grants that those snapshots already carry. One
  LICENSE file cannot state a date per commit. Rejected.
- **A shorter period of two or three years.** This stays possible later,
  because a Change Date can move earlier but never later. Not chosen now.
- **Commercial licenses for self-run Enterprise.** This contradicts the decision
  that Self-hosted runs Community only. Rejected.

## Consequences

- `enterprise/VERSIONS.toml` records three things per version: its first public
  commit, that commit's date and the Change Date. CI rejects a change that moves
  a recorded date later.
- Enterprise accepts external code contributions only with a CLA. Without one,
  Aidash could not run those contributions in production. Community accepts
  Apache-2.0 contributions with a DCO sign-off.
- No counsel reviewed these terms. The Licensor's merge of the cutover PR is
  the approval, and `docs/licensing.md` states this limitation.
- Revisions published before the cutover keep the terms they carried. The MIT
  notice covered only the Jev-derived portions. `aidash-broker` and
  `aidash-capability` were Apache-2.0.
- Nothing under `enterprise/` declares a license before the cutover: no
  `license` field, no SPDX identifier and no LICENSE file. A declaration that
  lands earlier would start the clock earlier than the date that is recorded.
  Until the cutover, the interim notice reserves all rights, and CI rejects any
  license declaration outside an allowlist.
- The Licensor and copyright holder is the pseudonym `kent8192`, identified only
  through that GitHub account. If an entity later takes over the rights, it
  becomes the Licensor of later versions only.
- Enterprise binaries and images are never distributed publicly; only Enterprise
  source is. Community images and desktop builds are published once releases
  begin.

## Amendment (2026-10-26)

This amendment refines the decision for the cutover (#255). It does not
supersede it.

- **Change Date.** The cutover PR fixes a date D in UTC, and the Change Date is
  D plus four years. For 0.1.0, D is 2026-10-26 and the Change Date is
  2030-10-26. The PR must merge on or after D. A later merge only shortens the
  period; a merge before D would exceed the four-year limit.
- **First public commit.** CI derives it from git as the commit that first
  added `enterprise/LICENSE`. A follow-up PR records its SHA and date in
  `enterprise/VERSIONS.toml`, and the history check compares that record with
  git. The cutover PR leaves the two fields out because its merge commit does
  not exist yet.
- **Interim guard superseded.** The consequence above that "CI rejects any
  license declaration outside an allowlist" no longer holds. #407 deleted the
  interim guard. #255 replaces it with checks for scope, license expressions,
  standard text integrity, BSL parameters, transition history, the dependency
  boundary, artifact contents and third-party attribution. Those checks do not
  scan prose that names a license, which is the rule that made #407 delete the
  old guard. The earlier consequence stays in this record as history.
