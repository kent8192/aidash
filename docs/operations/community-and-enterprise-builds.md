# Community and Enterprise builds

Aidash has two builds of one codebase (ADR 0032).

- The **Community build** is made from Community alone. It runs only the `self-hosted` Offering.
- The **Enterprise build** also contains `enterprise/`. It runs only the `cloud` and `dedicated` Offerings.

Each Deployment selects its Offering in the `[deployment]` settings table:

```toml
[deployment]
offering = "self-hosted"
```

When the table is absent, the Offering is `self-hosted`. Every command that loads settings refuses an Offering that its build does not run. It refuses before it connects to a database, and it names the Offerings that the build accepts.

| Runs                                                          | Build      | `offering`              |
| ------------------------------------------------------------- | ---------- | ----------------------- |
| Self-hosted Deployments, `deploy/helm/aidash`, `compose.yaml` | Community  | `self-hosted` (default) |
| Local development, Rust tests, acceptance, Bruno              | Community  | `self-hosted`           |
| Aidash's GCP Environments                                     | Enterprise | `cloud`                 |
| Local Enterprise development                                  | Enterprise | `cloud`                 |
| A customer's Dedicated Deployment                             | Enterprise | `dedicated`             |

## Layout

`enterprise/` is a separate Cargo workspace with its own `Cargo.lock`. Its `server` package composes the Community `aidash-server` library and passes its own build to it. No Community manifest, lockfile or root `Dockerfile` names `enterprise/`, so deleting the directory leaves a working Community tree.

`enterprise/infra/` holds the GCP Environments and delivery Terraform, the environment chart and the Cloud-only scripts. It is not part of the Community build.

`enterprise/` keeps copies of `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`, the `[workspace.package]` and `[workspace.lints]` tables and the profiles. `scripts/check-enterprise-alignment.py` fails when a copy differs from the Community file, or when `enterprise/Cargo.lock` holds a version of a Community-locked package that `Cargo.lock` does not hold. To fix a version difference, run the `cargo update --manifest-path enterprise/Cargo.toml -p <package> --precise <version>` command that the check prints.

Nothing under `enterprise/` declares a license until the licensing cutover (#255).

## Check the Community export

Run this from a clean commit. Docker must be running.

```sh
scripts/check-community-export.sh
```

The script does these steps:

1. It extracts `git archive HEAD` into a temporary directory, deletes `enterprise/` there, and refuses a tree that still has a top-level `infra/`.
2. It generates the OpenAPI description with `scripts/generate-api.sh` in the full tree's Community build and in the extracted tree. It fails unless the two files are byte-identical, and unless no path or tag names a Cloud-only feature (GCIP, the broker, billing, Secret Manager or Cloud KMS). `scripts/check-community-openapi.py` holds the denylist.
3. It runs `cargo metadata --locked` and `scripts/check-workspace-boundaries.py` in that tree.
4. It builds the root `Dockerfile` at the `runtime` target, which runs `cargo build --locked`.
5. It starts that image with `offering = "cloud"` and expects the Community refusal.
6. It starts the `dev` Compose profile's `backend` and waits until `/health` answers.

The CI job `community-export` runs the same script.

## Dashboard

The dashboard in `web/` is one single-page app. It reaches build-specific UI through extension slots. A slot is a module in `web/src/slots/` that `@slots/<name>` resolves to, and Community implements each as a stub.

| Slot           | Community stub                | Enterprise implementation                   |
| -------------- | ----------------------------- | ------------------------------------------- |
| `sign-in`      | No sign-in methods            | The GCIP sign-in page and its button label  |
| `account-menu` | No entries                    | The entry that starts two-step verification |
| `billing`      | No banner                     | Empty until billing (#242) fills it         |
| `sources.css`  | Adds no Tailwind source paths | Adds `enterprise/web/src` for Tailwind      |

The contracts are in `web/src/slots/types.ts`. The Enterprise build fails its type check when it lacks a slot or implements one differently.

`enterprise/web/` holds the Enterprise slots and the GCIP sources behind them. Its Vite configuration is the one in `web/vite.config.ts` with `@slots` pointed at `enterprise/web/src/slots`, so both builds read `web/index.html` and the same `node_modules`. `enterprise/web/package.json` declares no dependencies. Its `link` script makes `enterprise/web/node_modules` a link to `web/node_modules`, because packages must resolve from files outside `web/`, and every script runs it first. The GCIP SDK (`firebase`) stays in `web/package.json` and its lockfile, so that the dashboard keeps one dependency set. No Community module imports it, and `scripts/check-community-bundle.py` proves that the Community bundle does not contain it.

Build and test the Enterprise dashboard from the repository root:

```sh
npm ci --prefix web
npm run generate:api --prefix enterprise/web
npm run build --prefix enterprise/web
npm run test:gcip --prefix enterprise/web
```

`generate:api` exports the OpenAPI contract of the Enterprise server and generates the client in `web/src/generated/`. `enterprise/Dockerfile` runs the same build.

`scripts/check-community-bundle.py` rejects a Community bundle that holds Enterprise code. Pass it the output of `npm run build --prefix web -- --sourcemap`. It fails when a source map lists a file under `enterprise/`, when any file names a GCIP library or endpoint, or when the directory has no source maps. The CI job `community-bundle` runs it on the Community bundle, and the job `enterprise-web` runs it on the Enterprise bundle and expects both rules to reject it.

## Develop the Enterprise build locally

Build and test the Enterprise workspace from the repository root:

```sh
cargo test --locked --manifest-path enterprise/Cargo.toml
cargo metadata --locked --format-version 1 --manifest-path enterprise/Cargo.toml \
  | python3 scripts/check-workspace-boundaries.py --workspace enterprise
python3 scripts/check-enterprise-alignment.py
```

To run the Enterprise server against the services of `compose.yaml`, set `offering = "cloud"` in your local `server/settings/local.toml`. Then start it:

```sh
cargo run --locked --manifest-path enterprise/Cargo.toml --bin aidash -- serve
```

`compose.yaml` stays Community. To run the Community server again, remove the `offering` line.

## Images

The root `Dockerfile` builds the Community image. This change publishes it nowhere.

`enterprise/Dockerfile` builds the Enterprise image from the repository root. Its settings select the `cloud` Offering. `enterprise/infra/gcp/control/images.py` builds the `app` image of the GCP Environments from it, and the image goes only to Aidash's private Artifact Registry.

```sh
docker build -f enterprise/Dockerfile --target runtime .
```
