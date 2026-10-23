# Nonproduction delivery trusts the protected development branch

Decided on 2026-10-21 for #445. Nonproduction delivery took its trusted code from protected `main`, although its workflow YAML runs from the default branch `develop/0.1.0` and release candidates come from that branch too. When a release changed `infra/gcp/control/` or a privileged image (control, edge, observer), staging built them from the older `main` and production deployed that old bundle even though `tree(M) == tree(S)`. The new code reached production only with the second release, and staging never exercised it. Nonproduction workflows now check out one pinned commit of the protected automation branch as trusted code. A staging receipt is valid only when its `trusted_sha` equals its `source_sha`, so the staged controller, charts and privileged images come from S. Production admission accepts only a bundle whose `trusted_sha` equals the release source S with `tree(M) == tree(S)`, and applies with the checkout of S. Production keeps its own anchor: the dispatcher and the production notifications job check out `main`, and the production Workload Identity provider, Environment and `GITHUB_REF` checks do not change.

The `trusted_branch` configuration key stays, still `main`. It now names only production's code anchor, and no nonproduction check reads it.

## Considered Options

- **Keep the `main` anchor.** Privileged images lag one release behind the code that staging validated. The anchor is also not a boundary against `develop/0.1.0`, because the nonproduction provider already trusts `gcp-environments.yml@refs/heads/develop/0.1.0` and the `release-automation` and `nonproduction` Environments allow that branch, so a merge there can change the checkout ref. Rejected.
- **Build the privileged images from S but keep `main` orchestration.** The images would be current, but the controller and charts would still lag one release, and two anchors would remain to keep consistent. Rejected.
- **Anchor all nonproduction delivery on the protected development branch.** One anchor per tier, and the bundle that staging exercised is the bundle production promotes. Chosen.

## Consequences

- A merge into `develop/0.1.0` changes staging and PR preview behavior right away, instead of after a release. A bad controller change affects them before any release PR exists.
- Each develop merge changes the trusted revision, so the next requested PR or test deployment rebuilds all eight images. A retained release is deployed only by a controller from its own commit, so running environments keep running.
- Production gains no new trust. It rejects any bundle not built from S, and `main` must still be an ancestor of S, so `main` is merged back into develop after each release.
- ADR 0005's statement that the deploy pipeline is the trust root for Key Material still holds. The pipeline's entry point stays a protected GitHub Environment plus a Workload Identity Federation condition on protected refs, and the nonproduction condition already named the development branch.
