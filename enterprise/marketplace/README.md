# Standalone Marketplace

`backend/` is a Reinhardt REST project generated with `reinhardt-admin startproject`
and `startapp distribution --template rest`. `frontend/` is its independent Reinhardt
Pages browser project, generated with `--template pages` and an owning Pages app.
Both were generated from the pinned Reinhardt templates, then populated with the
existing protocol and application implementation.

The backend's `src/bin/manage.rs` runs the service's explicit `migrate`/`serve`
commands. Its `src/apps/distribution` owns handlers, storage and migrations;
`src/config` owns settings, installed apps and HTTP composition. Schema owners
migrate separately from runtime roles. The frontend's `src/client.rs` launches
Pages through `src/config/urls.rs` and its app's client router. It owns static
WASM assets, not a second backend or migration set.

See [backend setup](backend/README.md) and [frontend build](frontend/README.md).
The frontend has independent Cargo/npm lockfiles and an independently buildable
container. The native backends are named packages in `../Cargo.toml`; building
one with `-p` does not compile the Cloud server, worker or NATS.
