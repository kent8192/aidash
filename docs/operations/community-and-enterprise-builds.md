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

| Runs                                                        | Build      | `offering`              |
| ----------------------------------------------------------- | ---------- | ----------------------- |
| Self-hosted Deployments, default Helm chart, `compose.yaml` | Community  | `self-hosted` (default) |
| Local development, Rust tests, acceptance, Bruno            | Community  | `self-hosted`           |
| Aidash's GCP Environments                                   | Enterprise | `cloud`                 |
| Local Enterprise development                                | Enterprise | `cloud`                 |
| A customer's Dedicated Deployment                           | Enterprise | `dedicated`             |

The generic Helm chart also accepts a narrow public `deployment.settings`
descriptor for the Offering and an optional initial KMS key-version reference;
see [Deployment settings](../../deploy/helm/aidash/README.md#deployment-offering-and-initial-kms-key).
Its read-only JSON source, selected by `AIDASH_DEPLOYMENT_SETTINGS`, layers over
base/profile TOML for every server, worker and management settings consumer.
Managed GCP Environments supply Cloud explicitly. A Dedicated Deployment uses a
separate identity and the Enterprise image with `offering: dedicated`; switching
one retained Deployment between Offerings is unsupported.

The example profile leaves `personal_tenants` and `tenant_creation` unset so the
selected Offering supplies their defaults: both are enabled for Cloud and
disabled for Self-hosted/Dedicated. Explicit `[tenancy]` values still override
those defaults; other Tenancy settings retain their existing values.

## Layout

`enterprise/` is a separate Cargo workspace with its own `Cargo.lock`. Its `server` package composes the Community `aidash-server` library and passes its own build to it. No Community manifest, lockfile or root `Dockerfile` names `enterprise/`, so deleting the directory leaves a working Community tree.

`enterprise/` keeps copies of `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`, the `[workspace.package]` and `[workspace.lints]` tables and the profiles. Keep them equal to the Community files, and keep `enterprise/Cargo.lock` on the versions `Cargo.lock` holds for packages both lock (`cargo update --manifest-path enterprise/Cargo.toml -p <package> --precise <version>`).

Nothing under `enterprise/` declares a license until the licensing cutover (#255).

## Dashboard

The dashboard in `web/` is one single-page app. It reaches build-specific UI through extension slots. A slot is a module in `web/src/slots/` that `@slots/<name>` resolves to, and Community implements each as a stub.

| Slot           | Community stub                | Enterprise implementation                   |
| -------------- | ----------------------------- | ------------------------------------------- |
| `sign-in`      | No sign-in methods            | The GCIP sign-in page and its button label  |
| `account-menu` | No entries                    | The entry that starts two-step verification |
| `billing`      | No banner                     | Empty until billing (#242) fills it         |
| `sources.css`  | Adds no Tailwind source paths | Adds `enterprise/web/src` for Tailwind      |

The contracts are in `web/src/slots/types.ts`. The Enterprise build fails its type check when it lacks a slot or implements one differently.

`enterprise/web/` holds the Enterprise slots and the GCIP sources behind them. Its Vite configuration is the one in `web/vite.config.ts` with `@slots` pointed at `enterprise/web/src/slots`, so both builds read `web/index.html` and the same `node_modules`. `enterprise/web/package.json` declares no dependencies. Its `link` script makes `enterprise/web/node_modules` a link to `web/node_modules`, because packages must resolve from files outside `web/`, and every script runs it first. The GCIP SDK (`firebase`) stays in `web/package.json` and its lockfile, so that the dashboard keeps one dependency set. No Community module imports it.

Build and test the Enterprise dashboard from the repository root:

```sh
npm ci --prefix web
npm run generate:api --prefix enterprise/web
npm run build --prefix enterprise/web
npm run test:gcip --prefix enterprise/web
```

`generate:api` exports the OpenAPI contract of the Enterprise server and generates the client in `web/src/generated/`. `enterprise/Dockerfile` runs the same build.

## Develop the Enterprise build locally

Build and test the Enterprise workspace from the repository root:

```sh
cargo test --locked --manifest-path enterprise/Cargo.toml
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
