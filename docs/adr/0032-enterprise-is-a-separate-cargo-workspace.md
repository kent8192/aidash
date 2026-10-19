# Enterprise is a separate Cargo workspace that Community never references

Decided on 2026-10-13 (#124). Enterprise (BUSL-1.1) must be removable from the
source tree, so that a Community build can be made from Community alone. Cargo
cannot load a workspace whose manifests name a missing path. This holds for an
optional path dependency and for a glob workspace member alike (verified
empirically). ADR 0025's cloud adapters as Cargo features of the one server
therefore cannot survive removal of the Enterprise sources.

`enterprise/` is its own Cargo workspace instead. Its crates and the Enterprise
build's server binary depend on Community crates by path. No Community manifest
mentions `enterprise/`. The Community `server` becomes a library plus a thin
binary, so that the Enterprise binary can compose it.

## Considered Options

- **Cargo features with optional path dependencies in one workspace, rewriting
  the manifests when exporting Community.** The export would differ from what
  developers build, and `--locked` builds fail on the pruned lockfile. Rejected.
- **Feature-gated Enterprise modules inside Community crates.** One package
  would mix Apache-2.0 and BUSL-1.1 code. Rejected.
- **A separate repository.** This contradicts one codebase (ADR 0025). Rejected.

## Consequences

- This supersedes ADR 0025's "cloud adapters are Cargo features". There are two
  builds, the Community build and the Enterprise build. Cloud and Dedicated share
  the Enterprise build, so this is still not one binary per Offering. One
  codebase, one set of URLs and the validated runtime `offering` setting remain.
- There are two lockfiles. CI keeps the versions of shared dependencies aligned.
- Community defines the ports that Enterprise implements. Nothing in Community
  depends on Enterprise.
- Deleting `enterprise/` produces the Community source export.
- The Community build accepts only the `self-hosted` Offering. The Enterprise
  build accepts only `cloud` and `dedicated`. Aidash's Environments and local
  Enterprise development run as `cloud`.
- GCIP Tenants, Shared Pools, Tenant Bindings, Sign-in Domains and MFA
  Requirements move to Enterprise, in their types as well as their storage.
  Community has only a generic port saying that an External Identity is
  restricted to one Tenant. In the Community build that port never restricts.
- Enterprise tables come from the migrations of Enterprise Reinhardt apps.
  Foreign keys point only from Enterprise to Community. The GCIP parts of the
  identity migrations move to Enterprise, which the undeployed state allows.
- `web/` exposes extension slots that the Community bundle fills with stubs.
  The Enterprise build replaces those stubs with `enterprise/web/` through a
  Vite alias. CI rejects a Community bundle or source map that contains an
  `enterprise/` file.
- `infra/gcp`, `infra/delivery` and Cloud-only scripts move under
  `enterprise/`. `deploy/`, `runner/`, `desktop/`, `docs/` and
  `.github/workflows` stay Community. The OpenAPI description is generated and
  untracked. Each build generates a description of its own endpoints.
