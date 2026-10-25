# AGENTS.md

One full-stack auth Reinhardt Pages project generated with
`reinhardt-admin startproject auth --with-pages` and
`reinhardt-admin startapp authentication --with-pages`.

- Keep the generated project/app module boundaries: `src/apps/authentication/{client,server,server_fn,services,urls}` and shared serializers.
- Render views with `page!` and use lifecycle/state/resource/action hooks; keep reusable hooks in `client/hooks`. Use `form!` and `use_form` for validated forms.
- Pages components live in `client/components/`; native persistence, migrations and protocol adapters live in `server/`; services have target-specific modules.
- Keep `#[server_fn]` declarations target-neutral. Connect browser data and mutations through typed server functions with native request/service injection.
- Register the app once in `src/config/urls.rs`; keep the WASM launcher under `src/client/lib.rs` and native commands under `src/bin/manage.rs`.
- Build native and WASM from this single Cargo package and static assets from this directory, without Node.js. Bundle assets and the native executable in one product image.
- Preserve service-owned schema, owner/runtime credentials, migrations and Product Session authority. Never register a global database or run Cloud migrations.
- Keep Product tokens native; only opaque HttpOnly session cookies and CSRF proofs cross the browser boundary. GCIP stays Enterprise.
- Preserve the Claude-authored UI design. Use Rust 2024 sibling modules and Reinhardt ORM/migrations/Query; consult `instructions/`.
- Verify strict native/WASM Clippy, PostgreSQL/transport contracts, Pages builds and the Reinhardt wasm::e2e fixtures. Keep AGENTS.md/CLAUDE.md identical except the heading.
