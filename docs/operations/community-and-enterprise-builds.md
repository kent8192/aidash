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

`enterprise/` keeps copies of `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`, the `[workspace.package]` and `[workspace.lints]` tables and the profiles. `scripts/check-enterprise-alignment.py` fails when a copy differs from the Community file, or when `enterprise/Cargo.lock` holds a version of a Community-locked package that `Cargo.lock` does not hold. To fix a version difference, run the `cargo update --manifest-path enterprise/Cargo.toml -p <package> --precise <version>` command that the check prints.

Nothing under `enterprise/` declares a license until the licensing cutover (#255).

## Check the Community export

Run this from a clean commit. Docker must be running.

```sh
scripts/check-community-export.sh
```

The script does these steps:

1. It extracts `git archive HEAD` into a temporary directory and deletes `enterprise/` there.
2. It runs `cargo metadata --locked` and `scripts/check-workspace-boundaries.py` in that tree.
3. It builds the root `Dockerfile` at the `runtime` target, which runs `cargo build --locked`.
4. It starts that image with `offering = "cloud"` and expects the Community refusal.
5. It starts the `dev` Compose profile's `backend` and waits until `/health` answers.

The CI job `community-export` runs the same script.

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

`enterprise/Dockerfile` builds the Enterprise image from the repository root. Its settings select the `cloud` Offering. `infra/gcp/control/images.py` builds the `app` image of the GCP Environments from it, and the image goes only to Aidash's private Artifact Registry.

```sh
docker build -f enterprise/Dockerfile --target runtime .
```
