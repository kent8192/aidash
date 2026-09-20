# Database migrations

This crate was scaffolded with `sea-orm-cli migrate init`. New migrations are scaffolded with `sea-orm-cli migrate generate <name>` and then maintained as Rust source using SeaQuery schema builders.

```sh
sea-orm-cli migrate up
sea-orm-cli migrate status
sea-orm-cli migrate down
```

Set `DATABASE_URL` for CLI commands. Application startup runs the same `Migrator`. PostgreSQL runs each migration batch in a transaction. Fresh schemas support down/up verification. Existing SQLx migration records are adopted after successful status and SHA-384 checksum validation; adoption preserves application data and rejects rollback of the adopted versions.
