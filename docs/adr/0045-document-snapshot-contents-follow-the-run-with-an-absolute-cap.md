# Document snapshot contents follow their Run, with an absolute cap

Decided on 2026-10-15 for #225. Runs are never deleted, so keeping Research evidence "with its Run" kept every Document snapshot's full page text forever. The contents of a Document snapshot are now kept only while they have a retention basis: the Run has not ended, or it ended less than a post-run period ago (default 24 hours). Independently of the Run, they are kept at most a maximum age after retrieval (default 7 days). An Evidence Revocation removes the basis at once. A sweeper on the existing retention loop removes contents without a basis and leaves a Purged snapshot: its identity, retrieval time, digests and the time and reason of the purge remain, so the Run's Evidence fragments, which keep the passages actually delivered, still support Source citations. Continuing to read or searching a Purged snapshot returns `snapshot_purged`; the model may read the Source again as a new, approved and charged attempt.

A Purged snapshot counts toward neither the Run's snapshot limit nor its snapshot byte limit, which bound what is stored; the fetch attempt limit still bounds how often a Run reads again.

The periods are Retention Rules in two layers. The Deployment's periods come from server settings, with code defaults that are the same in every Offering. Whoever runs the Deployment may set any period from zero to ten years; a change takes effect when the server restarts. They are also the longest any Tenant may keep. (Added 2026-10-15: these are deploy-time settings, not something Deployment Administrators change in the dashboard.) A Tenant may store shorter periods, changed by a subject its policy permits `retention.update` or by a Deployment Administrator, with each change kept in history. Every sweep applies the current periods to everything still kept.

## Considered Options

- **A clock from retrieval only (for example 24 hours).** A Run waiting three days for a person would lose its snapshots and pay to read them again. Rejected for the Run basis, kept as the cap.
- **The Run basis without a cap.** A Run that never ends would keep its snapshots forever. Rejected.
- **Pinning the periods at retrieval.** A Tenant that shortens its periods expects content it already holds to go. Rejected.
- **Retention settings per feature (in research package configuration).** Issue #225 asked for one retention basis per Tenant rather than per feature. Rejected.
- **Tenant periods without a Deployment ceiling, or allowed to lengthen.** A Cloud Deployment could not bound what it keeps. Rejected.
- **No purge (Deployment Administrators may choose "never").** The guarantee that page text does not stay forever would be optional. Rejected.
- **Deployment periods changed in the dashboard by Deployment Administrators, with history.** That needs a deployment-wide record, its history and an API for defaults that rarely change. Rejected in favor of server settings.
- **Counting Purged snapshots toward the snapshot limits.** A Run whose snapshots were purged could not read its Sources again. Rejected.
