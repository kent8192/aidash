# AGENTS.md

Standalone marketplace frontend generated with `startproject --template pages`
and `startapp catalog --template pages`.

- Keep Pages views, browser state and API adapters under `src/apps/catalog/client`.
- `src/client.rs` launches Pages. Aggregate app routes in `src/config/urls.rs`.
- Deploy static WASM assets independently. The sibling backend owns authorization, persistence, migrations and Product tokens.
- Maintain independent Cargo/npm lockfiles. Build without Cloud or its API generator.
- Keep GCIP in Enterprise; access/refresh tokens stay in backend. Use opaque HttpOnly cookies and CSRF proofs.
- Use Rust 2024 sibling modules and the Reinhardt facade; consult `instructions/` for Pages guidance.
- Verify native/WASM strict Clippy, `npm ci`, `npm run build`, and Playwright.
- Keep AGENTS.md and CLAUDE.md identical except their top-level filename.
