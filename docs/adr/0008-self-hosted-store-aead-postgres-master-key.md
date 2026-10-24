# Self-hosted Provider Credential Store is AEAD ciphertext in PostgreSQL under a Master Key supplied by whoever runs the node

Decided on 2026-10-09 (#157). A self-hosted node keeps Key Material and Plan Tokens in PostgreSQL, encrypted with AES-256-GCM under a key derived from a Master Key supplied by whoever runs the node. Each version gets a fresh random nonce, and the Tenant, the Store resource and the version number are bound as associated data, so ciphertext copied to another record or Tenant fails to decrypt. ADR 0002 rejected encrypted PostgreSQL rows for Cloud, and that rejection stands there. Self-hosted nodes have no second identity that could hold read access, and ADR 0006 removed environment-variable keys everywhere, so without a Store a self-hosted node could not call any model provider. `docs/protocol.md` states that this version is not a hostile multi-tenant sandbox, so the application host reading Key Material on a self-hosted node is accepted.

## Considered Options

- Secret Manager only, with self-hosted nodes keeping `credential_env`. Rejected by ADR 0006.
- A file-based store on the host. Rejected: it adds durable state beside PostgreSQL, complicates backups and multi-replica deployments, and isolates no better than ciphertext in the database with the key outside it.
- OS keychain on the desktop client. Rejected: the desktop client holds no provider tokens (decided 2026-10-09); the server stores and uses them.

## Consequences

- The Master Key comes from a file or an environment variable named in server settings. It is never an `AIDASH_SECRET_*` reference, because Registry configurations may name any `AIDASH_SECRET_*` variable and send its value as a bearer to a configured endpoint. The Fingerprint key leaves that namespace for the same reason, on Cloud as well.
- The application layer changes in two places: the Store's write operations take the Tenant (needed for associated data), and a narrow read port returns one pinned version. Cloud leaves the read port unset and keeps failing closed until the Credential Broker (#137) exists.
- The Master Key must be 32 random bytes. The encryption key and the key identifier are derived from it with HKDF-SHA256 under distinct labels; the identifier is never a plain hash of the Master Key.
- Every version row records the identifier of the Master Key that encrypted it. Settings accept one current Master Key and any number of retired keys for decryption only. Rotation is "add the new key, retire the old, restart"; a row moves to the new key when its Provider Credential is next rotated. A bulk re-encryption command is deferred.
- A node whose settings name a Store but whose Master Key is missing, unreadable or malformed refuses to start. The Store records every key identifier it has used, each with a key check value. A node whose configured keys include none of them refuses to start, which catches a wrong Master Key before it splits the Store; a key check value that fails to verify also refuses. When at least one configured key is known, rows encrypted under a key that is no longer configured stay in place with a startup warning; only their reads fail, so the Provider Credential can still be revoked and deleted.
- A recovery command (decided 2026-10-09) makes a Store usable again after keys are lost. It revokes the Provider Credentials whose pinned version became unreadable and removes the unreadable rows and key registrations, so the node can start under the new Master Key. Tenants then reconnect through OAuth.
- Plan Tokens (#159) live in the same tables under their own resource namespace. Subject ownership of a Plan Connection is enforced by its metadata, not by the ciphertext.
- Test-only registration of Provider Credentials belongs to #158. #157's tests drive the application service directly.
