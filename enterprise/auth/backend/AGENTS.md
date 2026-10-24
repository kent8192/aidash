# AGENTS.md

This is the independently deployable auth backend, generated with
`reinhardt-admin startproject --template rest` and `startapp authentication --template rest`.

- Keep protocol handlers, models, stores, tests and migrations under `src/apps/authentication`.
- Keep project configuration and HTTP composition under `src/config`.
- `src/bin/manage.rs` serves the service and runs its own explicit owner-credential migrations. Use `reinhardt-admin startapp <name> --template rest` from this directory to add apps.
- Preserve the separate schema, owner/runtime roles and migration ledger. Do not register a global database connection or run Cloud migrations.
- Keep shared domain/use cases in Community portable crates; keep GCIP in Enterprise.
- Use Rust 2024 sibling modules and the Reinhardt facade, ORM, migrations and Query. See `instructions/` for framework guidance.
- Verify with `cargo test --locked --manifest-path Cargo.toml` and strict all-targets Clippy; preserve the service protocol and PostgreSQL ownership tests.
- Track only example settings. Runtime credentials come from mounted files/environment.
- Keep this file and CLAUDE.md identical except their top-level filename.
