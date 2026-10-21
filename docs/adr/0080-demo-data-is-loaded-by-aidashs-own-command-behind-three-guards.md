# Demo data is loaded by Aidash's own command behind three guards

Decided on 2026-10-20 for #432. Environments and local development should start with Demo data: several Demo Tenants with fictional subjects, Registry contents and Catalog Approvals, and the real people who should find themselves inside them. Production must never receive it.

Aidash loads Demo data with its own management command, `aidash demo-data --from <structure> [--people <path>]`, which writes through Aidash's services inside a tenancy scope, as ADR 0040 requires for every write. In an Environment the controller runs it in the migration Job, after `migrate`. Locally a person runs it by hand. The structure lives in a checked-in file, and the real people come from a secret. A Deployment-wide record, written last, keeps the digest of the inputs: an unchanged input does nothing, a changed one adds only what is missing, and nothing is ever deleted.

Three guards keep it out of production:

1. The secret is a GitHub Environment secret of `nonproduction`, which production jobs cannot read.
2. The controller adds the command only to an Environment's migration Job, and gives an Environment built from a fork no people.
3. The command refuses to write unless the Deployment's `[demo_data] enabled` setting is true. It defaults to false.

## Considered Options

- **Reinhardt's `seed` and `loaddata` commands.** They open the ORM's global connection, which ADR 0040 removed, and `loaddata` writes rows without a scope or the domain's rules. Rejected.
- **Calling the HTTP API after deployment.** The controller would need an administrator's credentials, and the load would race the first requests. Rejected.
- **Loading on every deploy.** A long-lived develop Environment would undo what reviewers changed. Rejected.

## Consequences

- Demo data goes through the same services as an administrator's requests, so its Tenants get the same initial policies and Registry.
- A failed load leaves no record, so the next deploy retries it. Every step must be idempotent.
- Demo data holds no Provider Credentials, so ADR 0006 still holds: a person connects one through Provider Authorization before a Demo agent can run.
- Each Demo Tenant receives a default Node, and its Demo Catalog Approvals belong to that Node (#241). Registry references name the Deployment through `registry_deployment`; the structure uses `$deployment` for its id.
