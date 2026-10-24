# Nodes and Federation between Nodes

Design for [#241](https://github.com/kent8192/aidash/issues/241), prepared on 2026-10-19 against `feat/issue-244-remaining-sub-issues` at `2e8f5b454`. It settles every implementation choice that the #241 decisions, ADR 0028 and ADR 0064 leave open, so that the implementing phase makes no design calls. Section 10 lists what the decisions do not settle, each with a recommendation. Every `file:line` below was read at that commit. Paths are relative to the repository root. Revised on 2026-10-19 against `725de16d0` per the coordinator's review (`review-p6a.md`): Node disabling is removed, the key digest is the key ID, the open points of section 10 are settled as recorded there, and the migration names follow the base branch after #265.

Reconciled on 2026-10-24 with the accepted [Issue 244 completion design](issue-244-completion.md), [ADR 0083](../adr/0083-node-deletion-is-recoverable-and-releases-capacity-before-purge.md) and [ADR 0084](../adr/0084-normal-deployment-key-rollover-is-proven-by-the-currently-trusted-key.md). Recoverable deletion and trusted rollover supersede the original creation-only and single-key limits. The original migration, test and work breakdown sections remain historical implementation plans, not completion evidence. Sections 1.6, 4.4–4.5, 5.6 and 10 distinguish current controls, accepted targets and pending integration or enforcement.

The design assumes the end state of the phases that run beside it. #265 puts `tenant NOT NULL` on every Tenant-owned table and enforces its row-level policy. #252 moves GCIP, Secret Manager, KMS and the Credential Broker under `enterprise/` behind Community ports.

Vocabulary follows the glossary. A Deployment is one installation with the id `aidash://<name>`. A Node is an agent hub inside one Tenant. A Node name is assigned, unique in the Deployment and never reused. A Node label is free text. A Node address is `aidash://<deployment>/<node name>`. A Node Registration is a Tenant Administrator's decision that one of its Nodes accepts one named Node of another Tenant. A Peer Mapping maps one subject of a registered Node to a local subject. The Deployment Key signs Federation tokens.

## 1. Data shapes

### 1.1 Identity types in `aidash-domain`

A new module `crates/aidash-domain/src/nodes.rs`, with its tests in `crates/aidash-domain/src/nodes/tests.rs`, holds the types below. Nothing else in the workspace parses or formats a Node address.

```rust
/// `aidash://<name>`. The grammar is today's `validate_node_id`
/// (crates/aidash-domain/src/configuration.rs:4-17): 1 to 100 ASCII letters,
/// digits or hyphens after the scheme.
pub struct DeploymentId(String);

/// The same grammar and generator as `TenantName`
/// (crates/aidash-domain/src/identity/tenancy.rs:127-145): `<adjective>-<noun>-<8 chars>`.
pub struct NodeName(String);

/// `aidash://<deployment>/<node>`. `Display` prints that form. `FromStr`
/// accepts exactly one `/` after the authority and refuses a trailing path.
pub struct NodeAddress { pub deployment: DeploymentId, pub node: NodeName }

/// What a Node Registration knows about the other side.
pub enum Counterpart {
	SameDeployment,
	OtherDeployment { endpoint: String, key: PinnedKey },
}

/// The pinned Deployment Key of another Deployment.
pub struct PinnedKey { pub key_id: DeploymentKeyId, pub public_key_pem: String }

/// base64url without padding of SHA-256 over the 32 raw public-key bytes. 43 characters.
pub struct DeploymentKeyId(String);

pub struct Node {
	pub tenant: TenantName,
	pub name: NodeName,
	pub label: String,
	pub default: bool,
	pub created_at: DateTime<Utc>,
}
```

`DeploymentId::parse` replaces `validate_node_id`, which has about 30 callers. `NodeName::from_entropy` shares `words::ADJECTIVES`, `words::NOUNS`, `ALPHABET` and `RANDOM_LENGTH` with `TenantName`, so one word list serves both. `NodeAddress::MAX_LEN` is a constant derived from the two grammars, and `crates/aidash-domain/src/registry/rules.rs:147-152` uses it in place of the literal 108 when it bounds an agent's `id` plus `version`.

A `tenant` column plus a `node` column name a local Node. A column that may name a Node of another Deployment holds the full address as text and has no foreign key.

### 1.2 `[deployment]` replaces `[node]`

`NodeSettings` (`server/src/apps/federation/remote/serializers/settings.rs:12-36`, section `node`) is deleted. Its fields move into `DeploymentSettings` (`server/src/apps/operations/serializers/deployment.rs:5-11`), which already holds `offering`. The section keeps every runtime knob, so no second section appears.

```toml
[deployment]
offering = "self-hosted"
id = "aidash://brave-otter"            # was [node] node_id
endpoint = "https://aidash.example.com" # was [node] endpoint
api_token = "..."                       # unchanged
nats_url = "nats://127.0.0.1:4222"      # unchanged
web_dir = "../web/dist"                 # unchanged
lease_seconds = 30                      # unchanged
worker_count = 4                        # unchanged
background_enabled = true               # unchanged
# probe_listen                           # unchanged
federation = true                       # the Deployment-wide switch, section 1.7

[deployment.key]
file = "/var/lib/aidash/deployment-key.pem"  # Community: PKCS#8 PEM, Ed25519
# kms = "projects/.../cryptoKeyVersions/1"    # Enterprise only
```

`DeploymentSettings::validate` applies today's `validate_node_id` and `validate_endpoint` checks (`settings.rs:51-66`). The Community build refuses `[deployment.key] kms`, at startup, in the extension hook of section 1.6. Naming both `file` and `kms`, or neither, is refused when the settings load. The legacy environment names in `server/src/config/legacy_env.rs:42-50` become `AIDASH_DEPLOYMENT_ID`, `AIDASH_ENDPOINT` and `AIDASH_API_TOKEN`.

`Config.node_id` (`server/src/config/runtime.rs:7`), `Store.node_id` (`server/src/apps/execution/repositories/store.rs:61`) and `Registry::new(pool, node_id)` (`server/src/apps/registry/repositories.rs:79`) become `deployment: DeploymentId`. The 44 port accessors named `node_id`, `node`, `local_node` or `home_node_id` under `crates/aidash-application/src/ports/` split into `deployment()` where the value identifies the Deployment and an explicit `NodeAddress` parameter where the value identifies the Node of a Run, a Workspace or a grant. Section 2 lists each.

### 1.3 `nodes` and `node_names`

Two tables, because a Node's label and state are Tenant-owned data under row-level security (ADR 0040) while two facts must outlive the Tenant's purge. The name is never reused, and an inbound request must find the Tenant of its receiving Node without opening Deployment Scope (ADR 0047 says "from then on the receiving Node names it"). The Tenant root record (`tenants`, created in `server/migrations/identity/0002_tables.rs` since #265, ADR 0050) is the precedent.

`node_names`, Deployment-wide, created in `identity/0002_tables` right after `tenants`:

| Column       | Type                 | Notes                      |
| ------------ | -------------------- | -------------------------- |
| `name`       | text                 | primary key                |
| `tenant`     | text NOT NULL        | references `tenants(name)` |
| `created_at` | timestamptz NOT NULL |                            |

Unique `(name, tenant)` so `nodes` can reference both columns. Rows are never deleted. Purge leaves them.

`nodes`, Tenant-owned, created in the same migration:

| Column                | Type                 | Notes                                    |
| --------------------- | -------------------- | ---------------------------------------- |
| `tenant`              | text NOT NULL        | references `tenants(name)`               |
| `name`                | text NOT NULL        |                                          |
| `label`               | text NOT NULL        |                                          |
| `is_default`          | boolean NOT NULL     |                                          |
| `created_at`          | timestamptz NOT NULL |                                          |
| `created_by_kind`     | text NOT NULL        | same values as `tenants.created_by_kind` |
| `created_by_identity` | uuid                 | same as `tenants.created_by_identity`    |

Primary key `(tenant, name)`. Foreign key `(name, tenant)` to `node_names(name, tenant)`. Partial unique index `nodes_one_default` on `(tenant)` where `is_default`. The row-level policy comes from the rendered template like every Tenant-owned table (`server/src/database/policy.rs`).

The Node model lives in `server/src/apps/identity/models/nodes.rs`, beside `tenants`.

### 1.4 `node_registrations`

Replaces `peers` (`server/migrations/federation/0002_tables.rs:318-332`) in the same migration. One record serves two purposes. The owning Node accepts inbound exchange from the registered Node, and the owning Node addresses outbound exchange to it. Both are needed in practice, because a receiver calls its Home back over HTTP during admission (`crates/aidash-application/src/authorization/peer/admission.rs:140-163`), and that callback is itself inbound Federation at the Home.

| Column                     | Type                 | Notes                                                                    |
| -------------------------- | -------------------- | ------------------------------------------------------------------------ |
| `tenant`                   | text NOT NULL        | the owning Node's Tenant                                                 |
| `node`                     | text NOT NULL        | the owning Node's name                                                   |
| `source_node`              | text NOT NULL        | the registered Node's address                                            |
| `source_endpoint`          | text                 | null for a Node of this Deployment                                       |
| `source_public_key_pem`    | text                 | null for a Node of this Deployment                                       |
| `source_key_id`            | text                 | null for a Node of this Deployment                                       |
| `atomic_transactions`      | boolean NOT NULL     | replaces `atomic_peer_trust`, section 10                                 |
| `enabled`                  | boolean NOT NULL     |                                                                          |
| `revision`                 | bigint NOT NULL      | stale revisions answer 409                                               |
| `actor`                    | text NOT NULL        | `oidc:<identity>` or `deployment_administrator`, as peer mappings record |
| `created_at`, `updated_at` | timestamptz NOT NULL |                                                                          |

Primary key `(tenant, node, source_node)`. Foreign key `(tenant, node)` to `nodes(tenant, name)`. Check `(source_endpoint IS NULL) = (source_public_key_pem IS NULL) AND (source_public_key_pem IS NULL) = (source_key_id IS NULL)`. In Rust the three nullable columns parse into `Counterpart`. A same-Deployment registration whose `source_node` names no row in `node_names` is refused at creation.

`node_registration_history` mirrors `authorization_peer_mapping_history` (`federation/0002_tables.rs:190`), with the same columns plus `sequence` and without the primary key. Both tables are Tenant-owned and purged with the Tenant, as peer mapping history is today.

### 1.5 Peer Mapping key

`authorization_peer_mappings` (`federation/0002_tables.rs:200-216`) loses `source_tenant` and gains `node`:

| Column                                                        | Change                                          |
| ------------------------------------------------------------- | ----------------------------------------------- |
| `tenant`                                                      | unchanged, now also the receiving Node's Tenant |
| `node`                                                        | new, the receiving Node's name                  |
| `source_node`                                                 | now a Node address                              |
| `source_tenant`                                               | removed                                         |
| `source_subject`                                              | unchanged                                       |
| `credential_id`, `enabled`, `revision`, `actor`, `updated_at` | unchanged                                       |

Primary key `(tenant, node, source_node, source_subject)`, replacing `(source_node, source_tenant, source_subject)` (`federation/0003_keys_indexes.rs:89`). Foreign key `(tenant, node, source_node)` to `node_registrations`, so a Peer Mapping cannot outlive or precede its Node Registration. The history table changes the same way. `PeerMappingInput` and `Mapping` (`crates/aidash-domain/src/identity/peer_mapping.rs:9-26`) replace `source_tenant` with `node: NodeName` and type `source_node` as `NodeAddress`. #359's open question closes. The same source subject may be mapped at several receiving Nodes, and a conflict within one Node answers the neutral 409 that #359 decided.

### 1.6 The Deployment Key

**Port.** `crates/aidash-application/src/federation/key.rs`:

```rust
#[async_trait]
pub trait DeploymentKey: Send + Sync {
	fn key_id(&self) -> &DeploymentKeyId;
	/// SubjectPublicKeyInfo PEM, what `/.well-known/aidash` publishes.
	fn public_key_pem(&self) -> &str;
	/// Signs the exact JWS input, never a digest, like `aidash_capability::TokenSigner`.
	async fn sign(&self, input: &[u8]) -> Result<[u8; 64], SignError>;
}
```

The raw `DeploymentKey` interface remains the signer adapter. After the database pool exists, `bootstrap::initialize` constructs `ScheduledDeploymentKeys` and installs it in outbound `Egress.keys`. Every request and public identity read loads the authoritative schedule, so a management command is observed by all server and worker processes without a restart. `Config.deployment_key` retains the startup signer for availability checks; outbound signing and published identity use the ring. No configured key means this Deployment cannot federate with other Deployments. `aidash-application` gains a dependency on `aidash-capability`, a leaf crate with no Aidash dependencies, and so does `aidash-integrations`. Since #252 only the Enterprise workspace depended on `aidash-capability`, and `scripts/check-workspace-boundaries.py` let the application and harness layers depend on `aidash-domain` and each other only, so the script now also allows `aidash-capability` for those two layers. `Signing(&dyn DeploymentKey)` in the same module is the `TokenSigner` that mints Federation tokens with a Deployment Key.

**Self-hosted.** `FileDeploymentKey` in `crates/aidash-integrations/src/federation/key.rs` loads the PKCS#8 PEM named by `[deployment.key] file` with `ed25519_dalek::pkcs8::DecodePrivateKey`. A relative path starts at `core.base_dir`, like `web_dir`. Startup refuses a named file that is missing, unreadable, not a regular file, not Ed25519, or readable by group or others (checked on Unix only, on the opened handle). No `[deployment.key]` means `None`. Every start path calls `bootstrap::initialize`, so `serve`, `server`, `worker` and `runserver` all refuse alike.

`manage deployment-key generate [--force]` writes a new key to the configured path with mode `0600`, refuses to overwrite without `--force` (a replacement is written beside the file and renamed), and prints the key ID and the public key PEM. It refuses a `kms` key. `manage deployment-key show` prints the same for the configured key, file or KMS, through the loader that startup uses. Both read only `[deployment.key]` and `core.base_dir`, so they work before the rest of the settings or the database exist. The command lives in `server/src/apps/federation/peer/services/key_commands.rs` and registers in `bootstrap::management_commands()` (`server/src/bootstrap.rs:25-49`) like `node_commands::commands()`.

**Normal rollover (ADR 0084).** Keep the existing source intact and create a different owner-only Ed25519 PKCS#8 file, or an enabled Ed25519 Cloud KMS version. `manage deployment-key status` reads the authoritative current key and schedule. Publish a successor with:

```bash
manage deployment-key rollover --expected-current <current-key-id> --file /var/lib/aidash/deployment-key-next.pem
# Enterprise, with a complete immutable version resource name:
manage deployment-key rollover --expected-current <current-key-id> --kms projects/PROJECT/locations/LOCATION/keyRings/RING/cryptoKeys/KEY/cryptoKeyVersions/2
```

Relative successor file paths resolve against `core.base_dir` and are persisted as canonical absolute references. Every process must be able to read that same path. The command reads and validates the successor and checks signing access before writing a schedule. It does not create or enable KMS resources. Both scheduled commands need the database; `generate` and `show` retain their database-independent initial-source path. After scheduling, use `status` for the effective identity rather than `show`, which describes `[deployment.key]`.

The Deployment-wide `deployment_key_rollovers` row stores public identities, source references and one signed proof, never private PEM. Compare-and-set against the complete previous snapshot prevents concurrent normal rollovers and stale-current updates. Another rollover is refused until the predecessor's seven-day overlap ends. The exact schedule publishes now, activates after 24 hours and retires the predecessor seven days later. After activation every outbound token uses the successor; during overlap it carries one bounded `transition` proof. The existing 8192-byte total token limit and method, request target, query, body and lifetime bindings apply unchanged. An unavailable or changed successor source fails signing rather than falling back. File sources are reopened for each selection; immutable KMS versions cache their public-key adapter, while every signature still calls KMS.

`/.well-known/aidash` keeps `key` as the effective current public identity and adds optional `key_rollover` with `successor` before activation, `retiring` during overlap, publication/activation/retirement Unix seconds and the compact public proof. After retirement no predecessor key or proof is published. It exposes no source references, secrets or Nodes. Public discovery alone never changes receiver trust; the currently trusted key must authenticate the transition and the complete successor-signed request. Receivers that miss the whole overlap and emergency replacement require independent manual confirmation under their existing record authority.

**Manual receiving trust.** In Settings → Nodes, use **Manage key** on an existing foreign Node Registration. A separate **Deleted Node keys** panel lists catalog-backed deleted Nodes without restoring them. A Deployment Administrator can also manage an existing Peer Graph Grant's key in Trust → Authorization. Quarantine refuses new authenticated requests for that record. Ordinary transition proofs and permission edits cannot clear it. Obtain the expected Deployment Key ID through a separate trusted channel, then enter that ID and the Ed25519 public PEM and acknowledge the independent check. No public-identity fetch is required and no private key is accepted. Merely fetching a key or editing a registration is not independent confirmation.

The single-record GET/POST endpoints are `/api/tenants/{tenant}/nodes/{node}/registration-keys` (selector `source_node`) and `/api/authorization/{tenant}/peer-graph-grant-keys` (selectors `source_deployment`, `source_identity`). POST takes `action` (`quarantine` or `confirm`), `expected_keys_revision` and `expected_authorization_revision`; confirmation also requires `expected_key_id` and `public_key_pem`. Both revisions must match the existing record. HTTP 409 requires reviewing the fresh state and confirming again; the browser consumes its acknowledgement and never retries automatically. Only cryptographic state and its revision change: enabled state, permissions, authorization revision, actor timestamps and Graph cursors are preserved. Existing Tenant policy governs registration controls; Graph Grant controls retain their Deployment Administrator-only authority. Changes append finite administration audit events with the actual browser or bearer actor.

Ordinary authorization edits use the separate authorization revision. When the repeated key and endpoint select a preserved-key path, that input is not an expected-current-key precondition: an edit may succeed after a concurrent manual or normal key update while retaining the complete current trust, quarantine, key revision and endpoint. A different key at the initial read is still refused, and endpoint/discovery changes retain the locked current-trust check. Enabled foreign registration/grant requests still require endpoint and key ID; omitting them remains supported for disabling an existing record, not a new general permission-edit API.

This manual administration path requires the owning Node's current catalog record. Controls for a retained registration after physical purge/catalog expiry must use the exact retained-journal dependency helper under current Tenant policy and permanent Node names; that integration remains pending. The existing atomic recovery path can persist a verified normal key transition for exact already-admitted work at a closed Node or under a disabled registration. Identity owns the transaction that locks current policy and registration state, revalidates the participant, immutable manifest and caller, and compares both revisions before advancing trust. This grants no new authority, clears no quarantine and waives no proof expiry. Physical post-purge recovery, Run continuity after manual re-pin and live KMS timing remain separate acceptance gates; these administration controls do not establish them.

**Cloud and Dedicated.** `KmsDeploymentKey` (`enterprise/crates/aidash-gcp/src/capability/deployment_key.rs`) wraps `KmsTokenSigner`, which #252 moved to `enterprise/crates/aidash-gcp/src/capability.rs`, configured for the key version in `[deployment.key] kms`. It fetches the public key once per immutable version through KMS `GetPublicKey` and refuses a version whose algorithm is not `EC_SIGN_ED25519`, whose name differs, or whose `pemCrc32c` does not match. The Enterprise build installs it through a new async `Extension::deployment_key(kms)` hook in `server/src/bootstrap/build.rs`, beside `provider_credential_store`. The Community implementation of the hook refuses a `kms` key at startup with a message that names the Enterprise build.

**Key ID.** base64url without padding of SHA-256 over the 32 raw public-key bytes. It is the token `kid`, the value a Tenant Administrator types when registering a Node, and the value `/.well-known/aidash` publishes.

**Token.** `crates/aidash-capability/src/federation.rs` reuses the compact JWS code. `mint` (`crates/aidash-capability/src/lib.rs:110-122`) and the header and signature part of `verify` (`:151-178`) are generalized over the claims type and the `typ` string in a private `jws` module, and `Claims` keeps its own claim checks on top. The generalization is `crates/aidash-capability/src/jws.rs`, with the header, `mint`, `split` and `verify` that both token types share. `Failure` still maps a malformed token to `InvalidSignature`, so Capability Tokens behave as before. The Federation token has `alg = EdDSA`, `typ = "aidash-federation+jwt"`, `kid` = key ID, and these claims:

| Claim            | Value                                                                            |
| ---------------- | -------------------------------------------------------------------------------- |
| `iss`            | the sending Deployment id                                                        |
| `sub`            | the sending Node address                                                         |
| `aud`            | the receiving Node address                                                       |
| `subject`        | the sending subject's key in the sending Tenant's policy                         |
| `iat`, `exp`     | `exp - iat` at most 60 seconds, as `TOKEN_TTL_SECS`                              |
| `jti`            | a non-nil UUID                                                                   |
| `kid`            | the key ID, equal to the header                                                  |
| `protocol`       | `0.4`                                                                            |
| `method`, `path` | the request method and the request target, `/federation/v0.1/...` with its query |
| `body`           | base64url without padding of SHA-256 over the request body bytes                 |

`aidash-capability` cannot see the domain's protocol version, so the request carries it: `RequestBinding { protocol, method, path, body }`. `FederationClaims::unverified(token)` reads the claims of a token whose signature has not been checked, which is how the boundary adapter of section 3.3 finds `aud` and `sub` before it knows the key, and `FederationFailure` names each refusal. `FederationClaims::verify(token, keys, now, request)` checks, in order: length at most 8192, three parts, header `alg` and `typ`, known `kid`, signature with `verify_strict`, `kid` equal in header and claims, `exp > now`, `iat <= now`, `exp - iat <= 60`, non-nil `jti`, `protocol` equal to the current version, and `method`, `path` and `body` equal to the request. The receiver keeps no `jti` store. The binding to one request stops a captured token from reaching another endpoint, and a repeat of the same request within the window meets the handlers' existing idempotency, such as the per-source unique keys on admissions (`federation/0003_keys_indexes.rs:106,113`). Section 10 records that choice.

Every token is minted for one request, right before it is sent.

### 1.7 The Deployment-wide switch

`[deployment] federation = true` is the switch. It is a setting, like the Tenancy Rules, which are the `[tenancy]` section and change with a restart (`server/src/apps/identity/serializers/tenancy_settings.rs:1-2,17`), and like them a Deployment Administrator changes it in the settings. It defaults to `true` in every Offering. When it is `false`, the receiver refuses every token-bearing request, the outbound client refuses every target in another Deployment, and `/.well-known/aidash` still publishes the key. Section 10 names the alternative of a stored row with a Deployment Administrator route.

## 2. Column map

Reading this tree finds 24 live columns in 24 tables that hold a node id, not 22. The figure 22 counted `memory.home_node` and `semantic_agent_memory.home_node`, whose tables `knowledge/0012_discard_json_banks.rs:26-31` drops, and missed four columns under other names. Classifications cite `server/src/database/classification.rs`. "Pending(265)" tables become Tenant-owned through #265, so only the Node part is listed here.

A column that becomes a Node address keeps its name and changes its content. A column that becomes a local Node name is renamed `node` and gains the foreign key `(tenant, node)` to `nodes(tenant, name)`. A column that becomes a Deployment id is renamed `deployment`.

| Table.column                                     | Defined at                                    | Becomes                                                                                                                                                                                                                      | Classification                     |
| ------------------------------------------------ | --------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------- |
| `core_areas.home_node`                           | `execution/0002_tables.rs:34`                 | Node address of the Workspace's home Node                                                                                                                                                                                    | TenantOwned (`:47`), unchanged     |
| `events.node_id`                                 | `execution/0002_tables.rs:198`                | `node`, nullable: the Workspace's Node name, none for an event that concerns no Workspace. CloudEvent `source` (`crates/aidash-domain/src/entities.rs`) becomes the Node address, or the Deployment id when there is no Node | Pending(265) (`:62`)               |
| `generation_remote_dispatches.peer_node`         | `execution/0002_tables.rs:430`                | Node address of the receiving Node                                                                                                                                                                                           | Pending(265) (`:69`)               |
| `generation_remote_usage.dispatcher_node`        | `execution/0002_tables.rs:497`                | Node address                                                                                                                                                                                                                 | Pending(265) (`:72`)               |
| `generation_requests.home_node`                  | `execution/0002_tables.rs:556`                | Node address, never empty; plus a new `node` column. See below                                                                                                                                                               | TenantOwned (`:73`)                |
| `runs.home_node`                                 | `execution/0002_tables.rs:754`                | Node address of the Home; plus a new `node` column. See below                                                                                                                                                                | Pending(265) (`:93`)               |
| `decision_attempts.node`                         | `execution/0025_decision_evidence.rs:24`      | Node address of the executing Node. `server/src/apps/execution/repositories/decisions.rs:317` compares with the Run's own Node address                                                                                       | Pending(265) (`:56`)               |
| `atomic_authority_attempts.node_id`              | `federation/0002_tables.rs:18`                | Node address of the participant                                                                                                                                                                                              | DeploymentWide (`:97`), unchanged  |
| `atomic_participants.coordinator`                | `federation/0002_tables.rs:116`               | Node address                                                                                                                                                                                                                 | DeploymentWide (`:101`), unchanged |
| `atomic_peer_trust.node_id`                      | `federation/0002_tables.rs:137`               | removed with the table; `node_registrations.atomic_transactions` replaces it                                                                                                                                                 | removed                            |
| `atomic_votes.node_id`                           | `federation/0002_tables.rs:174`               | Node address                                                                                                                                                                                                                 | DeploymentWide (`:105`), unchanged |
| `authorization_peer_mapping_history.source_node` | `federation/0002_tables.rs:190`               | Node address; `node` added; `source_tenant` removed                                                                                                                                                                          | TenantOwned (`:106`)               |
| `authorization_peer_mappings.source_node`        | `federation/0002_tables.rs:204`               | Node address; key per section 1.5                                                                                                                                                                                            | TenantOwned (`:111`)               |
| `authorization_remote_admissions.source_node`    | `federation/0002_tables.rs:228`               | Node address of the Home; `node` added for the receiving Node; unique keys become `(tenant, node, source_node, grant_id)` and `(tenant, node, source_node, task_id)`                                                         | TenantOwned (`:112`)               |
| `authorization_remote_grants.node_id`            | `federation/0002_tables.rs:265`               | Node address of the receiving Node; `node` added for the Home Node that issued the grant                                                                                                                                     | TenantOwned (`:118`)               |
| `delegations.node_id`                            | `federation/0002_tables.rs:289`               | Node address of the target Node                                                                                                                                                                                              | Pending(265) (`:119`)              |
| `peer_events.node_id`                            | `federation/0002_tables.rs:310`               | removed with the table. No Rust code reads or writes it; only the model (`server/src/apps/federation/peer/models/peer_events.rs:6-13`) and a trigger (`federation/sql/forward/0006_triggers.sql:10`) name it                 | removed                            |
| `peers.node_id`                                  | `federation/0002_tables.rs:321`               | replaced by `node_registrations`                                                                                                                                                                                             | removed                            |
| `authorization_peer_graph_grants.source_node`    | `identity/0002_tables.rs:144`                 | `source_deployment`, a Deployment id, plus `source_key_id` and `source_public_key_pem`. Primary key `(tenant, source_deployment, source_identity)`                                                                           | TenantOwned (`:130`)               |
| `authorization_run_remote_reads.node_id`         | `identity/0002_tables.rs:272`                 | Node address of the Node whose Tenant's Registry was read                                                                                                                                                                    | Pending(265) (`:138`)              |
| `semantic_remote_operations.home_node`           | `knowledge/0002_tables.rs:235`                | Node address                                                                                                                                                                                                                 | Pending(265) (`:227`)              |
| `memory_participants.home`                       | `knowledge/0009_native_memory.rs:8`           | Node address of the Workspace's Node                                                                                                                                                                                         | TenantOwned (`:206`)               |
| `memory_banks.home`                              | `knowledge/0009_native_memory.rs:12`          | Node address; the unique index `memory_bank_scope` keeps its columns                                                                                                                                                         | TenantOwned (`:196`)               |
| `memory_learning_discovery.home`                 | `knowledge/0018_memory_control_receipts.rs:9` | `deployment`, because the engine job runs per Deployment (`server/src/apps/knowledge/repositories/engine_jobs/discovery.rs:20-42`)                                                                                           | DeploymentWide (`:203`), unchanged |

**`runs`.** Today `runs.home_node` holds the real local node id, and reads compare `run.home_node == self.node_id` (`server/src/apps/execution/repositories/store.rs:2979`, `server/src/apps/federation/remote/services/runtime.rs:338-340`). The column keeps that convention with Node addresses. A local Run's `home_node` is the address of its own Node. A new `node text NOT NULL` column names the executing Node, copied from the Workspace. A Run is local when `home_node` equals the address formed from `[deployment] id` and `node`. The local claim (`store.rs:1115,1174,1185`) binds the Workspace's Node address, and the peer-admitted insert (`server/src/apps/identity/repositories/peer_admission.rs:496,551`) binds the Home address from the admission. `runs_home_node_task_id_key` (`execution/0003_keys_indexes.rs:298-302`) is unchanged.

**`generation_requests`.** Today `''` means a local generation (`crates/aidash-application/src/generation/assignment.rs:270`), readers branch on `is_empty()` (`generation/lifecycle.rs:127`, `visibility.rs:10`, `publication.rs:43`) and SQL filters on `home_node = ''` (`server/src/apps/execution/generation/repositories/assignment.rs:524`, `foreign/maintenance.rs:105`). Two generated columns depend on the sentinel, `local_task_id` and `local_workspace_id` (`execution/0002_tables.rs:563-575`), and three foreign keys depend on them (`execution/0004_references.rs:197-222`). The sentinel goes. `home_node` always holds the Home's Node address, and a new `node` column names the requesting Workspace's Node. `local_task_id` and `local_workspace_id` become ordinary nullable columns that the application writes, set for a local request and null for a foreign one, with the check `(local_task_id IS NULL) = (local_workspace_id IS NULL)` and the three foreign keys unchanged. The partial unique index `generation_requests_home_task` and the partial constraint `generation_requests_local_task` (`execution/0003_keys_indexes.rs:506-526`) become one unique constraint on `(home_node, task_id)` and one on `(local_task_id)`. The `generation_foreign_binding` check (`execution/0002_tables.rs:581`) is rewritten against `local_task_id IS NULL`.

**Node-owned tables.** `workspaces` (`workspaces/0002_tables.rs`) gains `node text NOT NULL` with the foreign key `(tenant, node)` to `nodes`; implementing step 3 showed that the Workspace entity, events and Runs read the Node from the table they already read, so `authorization_workspaces` stays ownership only. The Workspace entity (`crates/aidash-domain/src/entities.rs:139-147`) gains `node: NodeName`. `runs`, `generation_requests` and `events` carry the copy described above. `authorization_catalog` and its history (`identity/0002_tables.rs:33-61`) gain `node`; section 5.4 gives the key.

**JSON and string sites.**

| Site                                                                                                                                                                                                                                                                                                                                                                                                 | Today                          | Becomes                                                                                                                                                                                                                                                                                                |
| ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | --- | ----------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `QualifiedRef.registry_node` (`crates/aidash-domain/src/registry/bindings.rs:390-397`) and `AgentNormalization.registry_node` (`:347-349`)                                                                                                                                                                                                                                                           | the Deployment's single id     | renamed `registry_deployment`, a Deployment id. The Registry is Tenant-owned and every Node of the Tenant uses it. The SQL checks `^aidash://[A-Za-z0-9-]{1,100}$` in `registry/sql/forward/0008_provider_descriptors.sql:6-7` and `0011_binding_contract.sql:4-6` rename the key and keep the pattern |
| `qualified_agent(node, id, version)` (`crates/aidash-domain/src/entities.rs:330-331`) and the SQL builders `(?                                                                                                                                                                                                                                                                                       |                                | '/agents/'                                                                                                                                                                                                                                                                                             |     | …)` (`server/src/apps/execution/generation/repositories/{assignment.rs:86,embedding.rs:55,inference.rs:34,reservation.rs:55,compaction.rs:56}`) | `{deployment}/agents/{id}@{version}` | `{Node address}/agents/{id}@{version}`. An agent subject belongs to one Node, and a subject chain that crosses Deployments stays unambiguous |
| `{node}/tasks/…`, `{node}/runs/…`, `{node}/human_requests/…` (`crates/aidash-application/src/generation/visibility.rs:16`, `crates/aidash-application/src/federation/foreign_reads.rs:38,43`, `crates/aidash-application/src/authorization/commands/effects.rs:45,76`, `server/src/apps/execution/repositories/decisions.rs:306`, `server/src/apps/identity/repositories/home_execution.rs:147,177`) | Deployment id prefix           | Node address prefix, because those resources belong to a Node-owned Workspace                                                                                                                                                                                                                          |
| `{node}/registry/…` (`crates/aidash-application/src/generation/reservation.rs:64,118`) and `Boundary.candidate_id` (`crates/aidash-domain/src/decision/evidence.rs:31-36`)                                                                                                                                                                                                                           | Deployment id prefix           | Deployment id prefix, unchanged, because the Registry is the Tenant's                                                                                                                                                                                                                                  |
| Foreign resource ids `<node-id>:<resource-uuid>` in transaction policies (`docs/transactions.md`, Authority and acceptance)                                                                                                                                                                                                                                                                          | Deployment id                  | Node address                                                                                                                                                                                                                                                                                           |
| `Intent.home_node`, `Intent.target_node` (`crates/aidash-domain/src/generation/intent.rs:11,15`)                                                                                                                                                                                                                                                                                                     | Deployment ids                 | Node addresses                                                                                                                                                                                                                                                                                         |
| NATS stream and subject names (`crates/aidash-integrations/src/nats.rs:29-33`, `server/src/apps/workspaces/sse/services/broker.rs:16-21`) and activation streams (`crates/aidash-integrations/src/activation.rs:42-54,211-214`)                                                                                                                                                                      | Deployment id                  | Deployment id, unchanged. Workers and NATS belong to the Deployment                                                                                                                                                                                                                                    |
| Memory recovery ledger key (`server/src/apps/knowledge/services/memory_recovery.rs:53,57`)                                                                                                                                                                                                                                                                                                           | Deployment id                  | Deployment id, unchanged                                                                                                                                                                                                                                                                               |
| `x-aidash-node` header (`crates/aidash-integrations/src/federation.rs:67`, parsed by `peer_node()` at `server/src/apps/identity/services/http_auth.rs:9-14`)                                                                                                                                                                                                                                         | the sending Deployment         | removed. The token names the sending Node                                                                                                                                                                                                                                                              |
| Request bodies `{"tenant":…,"subject":…}` on `scoped/discover` and `scoped/execution/inspect` (`docs/authorization.md:424,440`)                                                                                                                                                                                                                                                                      | the sending Tenant and subject | removed. The token's `subject` claim names the subject, and no Tenant is sent                                                                                                                                                                                                                          |
| Rate-limit key `peer:{node}` (`server/src/http.rs:359-366`)                                                                                                                                                                                                                                                                                                                                          | Deployment id                  | `node:{sending Node address}`                                                                                                                                                                                                                                                                          |
| Advisory lock `atomic:trust:{node_id}` (`server/src/apps/federation/transactions/models/atomic_peer_trust.rs:53`)                                                                                                                                                                                                                                                                                    |                                | removed with the table                                                                                                                                                                                                                                                                                 |

## 3. Protocol

### 3.1 Version and headers

`CURRENT` becomes `0.4` and `ACCEPTED_FROM` becomes empty (`crates/aidash-domain/src/federation/protocol.rs:9,12`). `compatible` therefore accepts only `0.4`, which meets the criterion "requests at protocol version 0.2 or 0.3 are refused". `carries_corpus_digest` (`:27-29`) and the branch at `server/src/apps/identity/services/remote.rs:161-164` are deleted, because every peer now carries the digest. The path namespace `/federation/v0.1` stays. The criterion tests the version header, and a new path would only add a 404 in front of the same refusal.

Request headers are `authorization: Bearer <Federation token>` and `x-aidash-protocol: 0.4`. `x-aidash-node` is gone. Response headers are unchanged (`server/src/error.rs:192,198,222`).

### 3.2 Endpoints

The 39 peer-authenticated routes stay at their paths. `AccessBoundary::peer()` (`server/src/apps/identity/services/boundary.rs:69-87`) remains the single entry, and it is the only place that reads the token. The routes are those registered in `server/src/apps/federation/transactions/urls.rs:11-16,31-38`, `server/src/apps/identity/urls.rs:97-106,117-118,127-128,161-162,169-184`, `server/src/apps/knowledge/urls.rs:27-30`, `server/src/apps/execution/capabilities/urls.rs:9-22` and `server/src/apps/execution/generation/urls.rs:9-20`, minus `GET /federation/v0.1/observe`, which only `GET /api/mesh` called and which goes with it (section 6).

`GET /.well-known/aidash` (`server/src/apps/federation/peer/views/management.rs:34`) returns `DeploymentIdentity`, which replaces `NodeIdentity` (`server/src/apps/federation/peer/serializers/identity.rs:6-12`):

```json
{
  "id": "aidash://brave-otter",
  "endpoint": "https://aidash.example.com",
  "protocol_version": "0.4",
  "key": {
    "id": "…",
    "public_key_pem": "-----BEGIN PUBLIC KEY-----\n…"
  }
}
```

`key` is `null` when the Deployment has no Deployment Key. `capabilities` and `clusters` are dropped. They described Nodes, and the document names no Node. `StateResponse.deployment` carries the same `DeploymentIdentity`, and the dashboard derives the Node it works in from the Tenant's Node list (section 11, last bullets).

New Tenant routes, all under `AccessBoundary::subject()` with the actions of section 4.3:

| Route                                                         | Action                     |
| ------------------------------------------------------------- | -------------------------- |
| `GET /api/tenants/{tenant}/nodes`                             | `node.read`                |
| `POST /api/tenants/{tenant}/nodes` body `{label}`             | `node.create`              |
| `PATCH /api/tenants/{tenant}/nodes/{node}` body `{label}`     | `node.update`              |
| `GET /api/tenants/{tenant}/nodes/{node}/registrations`        | `node_registration.read`   |
| `GET /api/tenants/{tenant}/nodes/{node}/registration-history` | `node_registration.read`   |
| `POST /api/tenants/{tenant}/nodes/{node}/registrations`       | `node_registration.update` |

The registration body is `{source_node, endpoint?, key_id?, atomic_transactions, enabled, expected_revision}`. For a Node of this Deployment, `endpoint` and `key_id` must be absent and `source_node` must exist in `node_names`. For a Node of another Deployment, both are required. The server fetches `{endpoint}/.well-known/aidash` with the 5 second timeout of today's `identity()` (`crates/aidash-integrations/src/federation.rs:74-88`), requires `id` to equal the address's Deployment, `protocol_version` to equal `0.4`, and `key.id` to equal the typed key ID, and stores the fetched PEM. A mismatch is refused with 422 and the reason. Registering a Node of the owning Tenant is refused, because the Tenant's policy decides within the Tenant.

The Peer Mapping routes keep their paths (`server/src/apps/identity/views/peer_mappings.rs:16,31,46`). The body gains `node` and loses `source_tenant`. `GET` takes `node` as a query parameter.

### 3.3 What the receiver checks, in order

One function decides every inbound exchange, whatever the transport:

```rust
// crates/aidash-application/src/federation/inbound.rs
pub struct Inbound {
	pub receiver: NodeAddress,
	pub sender: NodeAddress,
	pub subject: String,
	pub proof: Proof,
}
pub enum Proof {
	/// Built by the local transport inside this process. Trusted as an internal type.
	InProcess,
	/// Parsed but not yet verified; the key comes from the Node Registration.
	Token { raw: String, claims: FederationClaims },
}
pub enum Acting {
	SameTenant { subject: String },
	Mapped { credential_id: Uuid, subject: String },
}
pub struct Admitted { pub tenant: TenantName, pub receiver: NodeAddress, pub sender: NodeAddress, pub acting: Acting }

pub async fn accept(inbound: Inbound, request: &RequestDigest, ports: &InboundPorts) -> Result<Admitted>
```

The steps, each failing closed:

1. `inbound.receiver.deployment` equals `[deployment] id`. Otherwise refused.
2. `node_names[receiver.node]` names a Tenant. This read is Deployment-wide and opens the no-Tenant scope. Otherwise refused.
3. A transaction opens in `TenancyScope::Tenant(that tenant)`. The Tenant has no deletion record (`tenant_deletions::is_deleted`, `server/src/apps/identity/repositories/tenant_deletions.rs:93`). Otherwise refused. This fulfils the #239 requirement that a deleted Tenant's Nodes refuse Federation, and a restore needs no action because the rows stay.
4. The sender. If `sender.deployment` equals this Deployment, the proof must be `InProcess`, `node_names[sender.node]` must exist, and the Tenant it names must have no deletion record. Both reads are Deployment-wide (`tenant_deletions` and `tenants` are DeploymentWide, `server/src/database/classification.rs:189,191`), so the receiving Tenant's scope can make them. The receiver never reads the sender's `nodes` row, which its scope would hide. Otherwise the proof must be a token, `[deployment] federation` must be `true`, this Deployment must hold a Deployment Key, and `claims.iss` must equal `sender.deployment`. Any other combination is refused. A same-Deployment sender over HTTP is refused here, which is where the three self-node rejections of today (`crates/aidash-domain/src/identity/peer_mapping.rs:35`, `server/src/apps/identity/repositories/graph/persistence.rs:631`, `crates/aidash-application/src/federation/peers.rs:25`) end up.
5. Same Tenant. If the sender's Tenant equals the receiver's Tenant, `acting = SameTenant { subject }` and the function continues at step 7.
6. Other Tenant. `node_registrations[(tenant, receiver.node, sender)]` must exist and be enabled. For a sender in another Deployment, `FederationClaims::verify` runs with a `PublicKeys` holding the registration's key under its key ID, which also checks `kid` against the pin, the time window and the request binding. Then `authorization_peer_mappings[(tenant, receiver.node, sender, subject)]` must exist and be enabled, and its credential must be usable, through today's `credential_subject`, `begin_access`, `check_binding` and `lock_with_mode` (`server/src/apps/identity/repositories/peer_mappings.rs:110-164`, `server/src/apps/identity/services/access.rs:252-254`). `acting = Mapped`.
7. Policy. For `SameTenant`, `subject` must be a subject of the Tenant's policy and `node.use` on the resource `node` with id `receiver.node` must be allowed for it. For `Mapped`, the mapped subject's authority applies as today. In both cases the handler then requires the operation's own action, such as `federation.discover`, `federation.execute` or `federation.graph.read` (`crates/aidash-application/src/federation/registry_reads.rs:109`, `authorization/peer/execution.rs:46`, `server/src/apps/identity/services/peer/graph.rs:72`), for the acting subject. `node.use` is the gate, not the decision.

Steps 1 to 6, up to and including the registration and signature checks, answer one response, `401` with the body `{"error":"unauthorized"}` and nothing else, so that an unregistered caller cannot tell a wrong Node name from a wrong key. A missing or unusable Peer Mapping after a valid signature answers `403` as today (`crates/aidash-application/src/authorization/peer/mappings.rs:65-68`), and so does a policy denial.

The cross-Tenant mapping lookup runs in the receiving Tenant's scope. `DeploymentScopeReason::FederationInbound` (`server/src/apps/identity/repositories/peer_mappings.rs:99-103`, 12 production uses) is deleted. Policy context fields become `environment.transport = federation`, `environment.source_node`, `environment.source_scope` with the values `tenant`, `deployment` or `remote`, and the resource attributes `source_node` and `source_subject`. `source_tenant` disappears from both.

`AccessBoundary::peer()` becomes a thin adapter. It checks `x-aidash-protocol`, parses the bearer token into `FederationClaims` without verifying it, requires `claims.method`, `claims.path` and `claims.body` to match the request it is about to pass on, builds `Inbound` with `receiver = claims.aud`, `sender = claims.sub`, `subject = claims.subject`, calls `accept`, and stores `Admitted` in the request extensions in place of `AuthenticatedPeer` (`boundary.rs:84-86`). The 19 files that call `peer_node()` read the sender from `Admitted` instead. The per-sender rate bucket (`server/src/http.rs:357-362`) keys on `Admitted.sender`.

### 3.4 Exchange inside one Deployment

No HTTP, no token, the same `accept`. `LocalTransport` in `server/src/apps/federation/remote/services/local.rs` implements the five transport ports that `PeerHttp` implements today (`crates/aidash-integrations/src/federation.rs:91`, `crates/aidash-integrations/src/federation/authority.rs:11`, `crates/aidash-integrations/src/federation/registry_reads.rs:9`, `crates/aidash-integrations/src/federation/dependencies.rs:13`, `crates/aidash-integrations/src/federation/transactions.rs:12`). Each method builds `Inbound { proof: InProcess, sender, subject, receiver }` from the caller's authenticated context, calls `accept`, and then calls the same service function the HTTP view calls, with `Admitted` as its argument. The caller's own transaction is never open while the local transport runs. That is already how peer I/O is structured, since the application makes durable writes before peer I/O and the admission callback explicitly holds no transaction (`crates/aidash-application/src/authorization/peer/admission.rs:139`). `accept` opens its own transaction in the receiving Tenant's scope at step 3, which is why `node_names` is Deployment-wide and why one scope per transaction (ADR 0040) holds.

The sender's Node and subject in an in-process `Inbound` are trusted. They come from the Run's chain or the dashboard actor that the server already authenticated. Nothing else can construct `Proof::InProcess`, because the type is private to the application crate and only `LocalTransport` is given a constructor.

`Federation::request` (`crates/aidash-application/src/federation.rs:24-45`) picks the transport. A target whose `deployment` equals this Deployment goes to `LocalTransport`, and every other target goes to `HttpTransport`. A Node is never federated with itself. Same-Node work keeps today's local branches, such as `Home::local()` (`server/src/apps/federation/remote/services/runtime.rs:338-340`), with the comparison made on Node addresses.

### 3.5 Outbound to another Deployment

`HttpTransport` (today's `PeerHttp`, `crates/aidash-integrations/src/federation.rs:15-73`) takes the sending Node, the sending subject and the target Node. It loads `node_registrations[(sender.tenant, sender.node, target)]` for the endpoint and refuses an absent or disabled registration with `Error::Invalid`. It mints a token with the Deployment Key for the exact method, path and body, sends `{endpoint}/federation/v0.1{path}` with the two headers, and keeps today's timeouts (`:43-53`). When `[deployment] federation` is `false` or no key exists, it refuses before any network call with a typed `Error::FederationDisabled`, which maps to 503 and the code `federation_disabled`. The `Credentials` port keeps its runner use (`server/src/bootstrap.rs:1562`) and loses `PeerCredentials` (`:526-531`).

### 3.6 Peer Graph Grant

A Peer Graph Grant admits a named Deployment Administrator of another Deployment, who has no Node (ADR 0043). It is the one inbound caller that is not a Node, and it gets its own token shape on the one route that serves it, `POST /federation/v0.1/scoped/graph` (`server/src/apps/identity/services/peer/graph.rs:222-229`):

| Claim        | Value                       |
| ------------ | --------------------------- |
| `iss`, `sub` | the viewer's Deployment id  |
| `aud`        | the receiving Deployment id |
| `subject`    | `oidc:<identity uuid>`      |
| the rest     | as in section 1.6           |

`accept` takes a second entry, `accept_grant_viewer`, used only by that route when `aud` parses as a bare Deployment id. It requires `[deployment] federation` on, a key present, a `target_tenant` in the body that names a Tenant without a deletion record, an enabled `authorization_peer_graph_grants[(target_tenant, iss, identity)]` row, and a signature that verifies with the grant's receiving keyset (section 4.4). The grant row stores `source_key_id` and `source_public_key_pem`, typed and fetched at creation like a Node Registration. On the viewer's side, `POST /api/federation/graph` for a Deployment Administrator names `deployment` and `endpoint`, the server fetches the well-known document and requires the id to match, and `GET /api/federation/graph/peers` lists, for a Subject viewer, the Nodes registered by the viewer's selected Node, and nothing for a Deployment Administrator. Issue #301 has since decided to remove this special administrator path; [Issue #486](https://github.com/kent8192/aidash/issues/486) tracks that future change. The current server and dashboard still expose Peer Graph Grants.

## 4. Authority

### 4.1 Inside one Tenant

No Node Registration and no Peer Mapping. The receiving Node decides with the Tenant's policy over the sender's own subject, which is a subject of the same policy because the policy is Tenant-owned. The gate is a new action `node.use` on the resource kind `node`, whose id is the Node name and whose attributes are `label` and `default`. The initial policy (`crates/aidash-domain/src/identity/tenancy.rs:258-311`) adds `node.read` and `node.use` to `MEMBER_ACTIONS` (`:33-118`). `members` therefore hold both, `administrators` hold `*`, and an approved agent joins `members` by default (ADR 0062, `default_agent_groups` at `crates/aidash-domain/src/identity/tenant_administration.rs:217-222`), so the Tenant's subjects use all of its Nodes until a Tenant Administrator narrows the rule, for example with a condition on `resource.name`.

`node.use` also gates creating a Workspace in a Node (section 5.5). The operation's own action is still required after the gate (section 3.3, step 7).

### 4.2 Across Tenants

A Node Registration at the receiving Node for the sending Node, and a Peer Mapping keyed by `(receiving Node, sending Node, sending subject)`. Both are the receiving Tenant Administrators' decisions, so a Tenant never needs a Deployment Administrator to federate (#363). A same-Deployment registration pins no key. Whether the sending Deployment is this one or another, the acceptance code is the same function.

A Tenant Administrator binds a Peer Mapping only to a credential it could issue now, as #359 decided, and that rule is unchanged.

### 4.3 Actions

New constants in `crates/aidash-domain/src/identity/tenant_administration.rs`, added to `ACTIONS` (`:39-59`), to `ADMINISTRATION_ACTIONS` (`:67-78`) where marked, and to `resource_kind` (`:93-111`). `docs/authorization.md` gains the rows.

| Action                                     | Resource kind       | Default holders             | Reported by `GET /api/tenants/{tenant}/administration` |
| ------------------------------------------ | ------------------- | --------------------------- | ------------------------------------------------------ |
| `node.read`                                | `node`              | `members`, `administrators` | no                                                     |
| `node.use`                                 | `node`              | `members`, `administrators` | no                                                     |
| `node.create`                              | `tenant`            | `administrators`            | yes                                                    |
| `node.update`                              | `node`              | `administrators`            | yes                                                    |
| `node.delete`                              | `node`              | `administrators`            | yes, added under #388                                  |
| `node_registration.read`                   | `node_registration` | `administrators`            | yes                                                    |
| `node_registration.update`                 | `node_registration` | `administrators`            | yes                                                    |
| `peer_mapping.read`, `peer_mapping.update` | `peer_mapping`      | `administrators`            | yes, unchanged from #363                               |

`peer_mapping.*` keep their names and scope. A Peer Mapping now names a Node, so the resource attributes gain `node`. `SubjectKind::Node` (`crates/aidash-domain/src/policy.rs:29-34`) has no producer in the tree and is deleted. A Node is a resource, never a subject.

### 4.4 A Peer Graph Grant pins the key

Section 3.6. The grant starts with the viewer Deployment's independently selected key ID and fetched public PEM. Under ADR 0084, its receiving keyset can advance through a currently trusted predecessor's proof after the complete viewer request and current authority pass verification. A normal rollover does not require re-creating the grant or changing its authorization revision, enabled state, permissions or Graph cursors. Missed overlap or lost/compromised keys require a receiving Deployment Administrator's independent manual confirmation; quarantine cannot be cleared by a normal proof or an ordinary permission edit. Section 1.6 describes the existing manual controls and their separate key and authorization revisions. The future removal decided by #301 and tracked in #486 is not implemented by these key controls.

### 4.5 A deleted Tenant's Nodes

Step 3 and step 4 of `accept` refuse inbound exchange to and from a Node whose Tenant has a deletion record. Workspace creation in such a Tenant already fails at `Access::begin` through `lock_with_mode` (`server/src/apps/identity/services/core.rs:108-124`). Running work is paused by the deletion itself (`server/src/apps/identity/repositories/tenant_deletions.rs:224-239`). Tenant restore reopens preserved Node rows under current authority without restarting canceled work; a Node's own unresolved deletion still keeps that Node closed. Tenant purge deletes `nodes`, `node_registrations`, `node_registration_history`, Peer Mappings and their history with the Tenant's other Tenant-owned rows (ADR 0061) and leaves `node_names`.

Retained audit creates no read authority. Issue #301's P15 decision closes ordinary audit APIs while the Tenant is deleted or purged; a valid restore re-enables ordinary Tenant-authorized reads, and retained history expires under `audit_retention_days`. This is an accepted authority boundary, not evidence that every current runtime path enforces it. [Issue #484](https://github.com/kent8192/aidash/issues/484), coordinated with [Issue #368](https://github.com/kent8192/aidash/issues/368), tracks enforcement. See [Tenant purge and retained audit](../authorization.md#purge).

## 5. Lifecycle

### 5.1 Every Tenant gets a default Node

`tenants::create` (`server/src/apps/identity/repositories/tenants.rs:188-247`) calls a new `nodes::create_default(tx, &name, creator)` right after `insert_root` (`:196`) and before the policy is written. `create_default` draws a `NodeName` from a random UUID's bytes, inserts `node_names` and `nodes` with `label = "Default"`, `is_default = true` and the creator's kind and identity, and retries up to 8 times on a name collision like `insert_root` (`:137-183`). Personal Tenants go through the same `create` (`server/src/apps/identity/repositories/personal_tenant.rs:107`), and so do Deployment Administrator-created Tenants (`server/src/apps/identity/services/tenants.rs:166,233`).

The test fixtures that insert root rows without `tenants::create` call `create_default` too. They are `tenant_root` (`server/src/apps/execution/tests/support/tenant_roots.rs:7`), `tenant_root_in` (`:16`) and `tenant_scope` (`:46`). Each returns or exposes the default Node's `NodeAddress`, and tests that today use `store.node_id` as a Run's home use that address.

### 5.2 `node.create` and "Nodes per Tenant"

`TenancyRules` (`crates/aidash-domain/src/identity/tenancy.rs:182-203`) gains `nodes_per_tenant: u32`, default 10. `[tenancy] nodes_per_tenant` maps it (`server/src/apps/identity/serializers/tenancy_settings.rs:41`) and the validation accepts 1 to 1000 (`:54-62`). `docs/authorization.md:108-115` gets the row. Every Offering defaults to 10, and the Tier may be stricter.

`POST /api/tenants/{tenant}/nodes` requires `node.create` on the Tenant resource, takes a per-Tenant advisory lock `nodes:<tenant>`, counts the Tenant's live Nodes, and refuses when the count is at or above `min(config.tenancy.nodes_per_tenant, tier limit)`. The refusal is `403` with the code `node_limit_reached`, beside `tenant_limit_reached` (`server/src/error.rs:10-11`). The Tier limit comes through a port `TierLimits::nodes(tenant) -> Option<u32>` in `aidash-application`, whose Community implementation returns `None`. #242 supplies the Enterprise implementation and the Free Tier's 1, and refuses a Tier change that would exceed the limit, as its decisions say. The handler then draws a name, inserts both rows and records an `AdministrationEvent` with action `node.create`, like `tenant.create` (`tenants.rs:229-247`).

A Tier change locks the Billing Account and each covered Tenant's `nodes:<tenant>` capacity lock in name order, validates current counts, and retains those locks until the payment adapter publishes the new locally readable Subscription. Attachment reads the current Subscription and Node count under the same locks before it commits.

`PATCH /api/tenants/{tenant}/nodes/{node}` requires `node.update`, changes the label and records the actor's `node.update` administration event in the same transaction. There is no revision column; the label is a last-write-wins field. Section 5.6 describes the lifecycle added under #388.

### 5.3 Built-in seeding stays per Tenant

`seed_tenant(tx, node, tenant)` (`server/src/apps/registry/repositories.rs:288-299`) and `Registry::seed_system` (`:60-69`) take the `DeploymentId`, and `core_descriptor` (`crates/aidash-domain/src/tool/providers.rs:194-203`) writes it into `registry_deployment`. Nothing about seeding depends on Nodes, because the Registry stays Tenant-owned (#241, "What belongs to a Node"). The byte comparison in `crates/aidash-application/src/registry/system.rs:73-79` keeps working because every Tenant seeds with the same Deployment id.

### 5.4 Catalog Approvals name a Node

`authorization_catalog` gains `node text NOT NULL`. Primary key `(tenant, node, entry_id, entry_version)` replacing `(tenant, entry_id, entry_version)` (`identity/0003_keys_indexes.rs:34`), foreign key `(tenant, node)` to `nodes`. The history table gains the same column. `TenantCatalog { node }` (`server/src/apps/identity/services/policies.rs:124`) becomes `{ deployment, node }`. `POST /api/authorization/{tenant}/catalog/approvals` takes `node` in the body, `POST /api/authorization/{tenant}/catalog` replaces one Node's catalog and names it in the body, and `GET` takes `node` as a required query parameter.

`approve` (`crates/aidash-application/src/authorization/catalog/approvals.rs:52-137`) builds the subject with `qualified_agent(&NodeAddress { deployment, node }, id, version)` (`:120`), so an approved agent's subject is keyed by the Node it was approved on, as ADR 0062 requires per version and this design requires per Node. Generation, which registers, approves and adds the subject together, approves on the requesting Workspace's Node (`generation_requests.node`). Built-in definitions that need approval (Sandboxed ones, Research providers and Deciders) are approved per Node. A new Node starts with no approvals. Section 10 notes the usability consequence.

`GET /api/research/providers?node=<name>` requires `node.use` on the owned Node and reports that Node's approved Research definitions. Omitting `node` selects the Tenant's default Node. The dashboard includes the selected Node in the request and its query cache.

The file-share selector combines the Tenant's Nodes with the distinct Node addresses returned by discovery for the sending Run's Node. `GET /api/file-recipients?node_id=<address>&run_id=<run>` authorizes the sending Run and its executing Node, then uses that Node's registrations and Federation identity for the remote lookup. Omitting `run_id` keeps the default-Node lookup for clients with no sending Run.

Transaction preflight validates each Workspace's Node, each Task's Workspace Node and each Run's executing Node against the exact participant address. Reservation validation and mutation application repeat that stored-ownership check in their original transaction.

### 5.5 Workspace creation picks a Node

`WorkspaceInput` (`server/src/apps/workspaces/serializers/management.rs:10-17`) gains `node: Option<NodeName>`. `Access::create_workspace` (`server/src/apps/identity/services/workspace.rs:30`, `crates/aidash-application/src/workspaces/mutations.rs:6-21`) resolves the default Node when absent, requires `workspace.create` as today and `node.use` on the chosen Node, and writes `node` into `authorization_workspaces` (`server/src/apps/execution/repositories/store.rs:688-714`). Tasks, Runs and Messages belong to the Workspace's Node. The local claim copies `node` onto the Run (section 2).

The dashboard adds a Node selector to the Tenant shell beside the Tenant switcher (`web/src/shell/account-menu.tsx`), remembered per Tenant in local storage and defaulting to the default Node. The Workspace list shows the selected Node's Workspaces. `WorkspaceForm` (`web/src/forms.tsx`) gains a Node select preset to the selection. The selected Node is the sending Node for everything a person starts from the dashboard, such as discovery and Graph View. Node management and Node Registrations get a page under the Tenant's settings, and `web/src/peer-mappings.tsx` gains the Node.

### 5.6 How a Node stops

ADR 0083 and the completion design extend the original creation-only boundary with recoverable deletion; there is no separate disabled Node state. The current lifecycle API supports default switching, deletion, restore and a purge request. After `/api/tenants/{tenant}/nodes/{node}`, the POST suffixes are `/default` under `node.update`, `/deletion` and `/restore` under `node.delete`, and `/purge` for a Deployment Administrator only. These mutations use the current authorized browser Mapping of the path Tenant or Deployment Administrator authority, and record the actual actor. See the [lifecycle management contract](../authorization.md#tenancy-rules-tenant-creation-and-invitations) for request bodies, events and listing authority.

Deletion requires the exact Node name as confirmation. Move its default role to another live Node first; the default or last live Node cannot be deleted. Deletion fixes `purge_after` using `node_deletion_grace_days` (30 days by default, range 1–90), releases its Node-count slot immediately and closes ordinary admission and access at the next durable boundary. Restore checks current capacity, policy and credential/registration state, preserves stop intent and never restarts canceled work or restores revoked authority. It is refused after the deadline or once purge has begun. Only a Deployment Administrator restores a Node a Deployment Administrator deleted. Creation, default changes, deletion and restore share the Tenant capacity lock.

Settings → Nodes presents lifecycle controls alongside registration key controls. Lifecycle updates refresh the separate Deleted Node keys list, and restoring a Node clears that panel's old selection. The ordinary Node selector remains live-only; the paginated management list includes deletion state and deadline. The current `NodeRecord.storage_retained_until_purge` is a lifecycle boolean, not a measured byte count or a price.

The accepted purge target erases only Node-owned data, preserves sibling Nodes and Tenant-shared data, retains permanent Node names and keeps separable audit for its finite retention period. Retained files remain in the active Tenant's storage meter until physical erasure, subject to the Offering and the existing Tenant-deletion exemption; Pod time ends on actual stop. The physical purge worker, measured storage bytes and additional non-atomic terminal recovery receipts still await source integration. A successful `/purge` currently records a request, not evidence of physical erasure. The implemented exact atomic recovery/key path in section 1.6 does not establish those pending gates. See [Node deletion and recovery](../protocol.md#node-deletion-and-recovery) for the accepted cleanup and retention boundaries.

## 6. Removals

Delete, with no compatibility shim:

- `peers`, `peer_events` and `atomic_peer_trust`, their models (`server/src/apps/federation/peer/models/{peers,peer_events}.rs`, `server/src/apps/federation/transactions/models/atomic_peer_trust.rs`), `PeerRecords` (`server/src/apps/federation/peer/models/records.rs`), the write-guard trigger line in `federation/sql/forward/0006_triggers.sql:10`, and the row mapping at `server/src/database/records.rs:159`.
- `Peer` (`crates/aidash-domain/src/federation.rs:11-17`), `PeerAuthority` and `require_distinct_credentials` (`crates/aidash-application/src/federation/peers.rs`), the ports `PeerConfiguration` and `PeerIdentity`, `PeerCredentials` (`server/src/bootstrap.rs:526-531`), `peer_secret` and `secret` (`server/src/apps/federation/remote/services/runtime.rs:131-143`), `validate_peer_credential` and the `AIDASH_SECRET_` name rule (`crates/aidash-domain/src/configuration.rs:52-72`) where no runner use remains, and `same_secret` if it loses its last caller.
- `credential_env` everywhere it names a peer. `scripts/test-rust.sh:10-14` and `scripts/test-transactions.sh:10` stop exporting `AIDASH_SECRET_TEST_PEER`, `AIDASH_SECRET_GRAPH_THIRD` and `AIDASH_SECRET_TRANSACTION_NN`.
- The Deployment Administrator peer routes `POST /api/peers` (`server/src/apps/federation/peer/views/management.rs:12-18`), `GET /api/mesh` (`:29-32`) with `GET /federation/v0.1/observe` (`:39`), `GET` and `POST /api/transactions/trust` (`server/src/apps/federation/transactions/views/protocol.rs:70,79`) and `POST /api/transactions/peer-recovery` (`:146`), with their services and the `peer.registered` event.
- In the dashboard, `PeerForm` (`web/src/forms.tsx:684-729`, which also hardcodes protocol `0.2`) and its callers in `web/src/collaboration/details.tsx:201` and `web/src/collaboration/settings.tsx:422`, the mesh view in `web/src/main.tsx`, and the trust controls in `web/src/transactions.tsx`.
- Protocol `0.2` and `0.3`, `ACCEPTED_FROM` entries, `carries_corpus_digest`, the per-peer `protocol_version`, and `x-aidash-node` with `peer_node()`.
- The self-node rejections (section 3.3, step 4).
- `DeploymentScopeReason::FederationInbound` and `SubjectKind::Node`.
- `NodeSettings`, `AIDASH_NODE_ID`, `validate_node_id` as a free function, and `NodeIdentity`.
- The `''` sentinel in `generation_requests.home_node` and the two generated columns.
- `source_tenant` in Peer Mappings, their history, the protocol bodies and the policy context.
- `Pending(241)` from `Classification` (`server/src/database/classification.rs:14-17`) and from the test `pending_tables_name_a_step_of_the_rollout` (`server/src/database/classification/tests.rs`).

## 7. Migration plan

Aidash is undeployed, so each table is edited where it is created, and its state snapshot and model struct change with it. After every edit, `debug/manage makemigrations --state-source files --dry-run --check` from `server/` prints "No changes detected", and the counts in `server/src/apps/execution/tests/migrations.rs` match `grep -rl '.state_only(true)' server/migrations | wc -l`. No new migration file is added, so `execution/0044_policy_probe` keeps its name and dependency, and `TENANT_REGISTRY_MIGRATIONS` (`migrations.rs:36`) is unchanged.

| Migration                                                                                                   | Edit                                                                                                                                                                                                                                                                                                                                 |
| ----------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `identity/0002_tables.rs`                                                                                   | create `node_names` and `nodes` right after `tenants`, which #265 moved into this migration; their keys ride in `0003_keys_indexes`, their foreign keys in `0004_references` and their state in `0007_model_state` like every other table of the app                                                                                 |
| `identity/0002_tables.rs`, `0003_keys_indexes.rs`, `0007_model_state.rs`                                    | `authorization_workspaces.node`; `authorization_catalog` and history `node` and primary key; `authorization_peer_graph_grants` columns and primary key                                                                                                                                                                               |
| `federation/0002_tables.rs`, `0003_keys_indexes.rs`, `0004_references.rs`, `0007_model_state.rs`            | `peers` becomes `node_registrations`; `node_registration_history` added; `peer_events` and `atomic_peer_trust` removed with the foreign key at `0004_references.rs:26-37`; `authorization_peer_mappings` and history re-keyed; `authorization_remote_admissions` and `authorization_remote_grants` gain `node` and their keys change |
| `federation/sql/forward/0006_triggers.sql` and its backward asset                                           | drop the `peer_events` guard                                                                                                                                                                                                                                                                                                         |
| `execution/0002_tables.rs`, `0003_keys_indexes.rs`, `0004_references.rs`, `0007_model_state.rs`             | `runs.node`; `events.node`, and `events.tenant` becomes NOT NULL because the `peer.registered` event, the only Tenant-less one, goes with `peers`; `generation_requests.node`, the two `local_*` columns, the check and the two unique constraints                                                                                   |
| `knowledge/0018_memory_control_receipts.rs` and its state                                                   | `memory_learning_discovery.deployment`                                                                                                                                                                                                                                                                                               |
| `registry/sql/forward/0008_provider_descriptors.sql`, `0011_binding_contract.sql` and their backward assets | `registry_deployment` in the JSON checks                                                                                                                                                                                                                                                                                             |
| `identity/0032_tenant_owned_row_level_security`, `federation/0011_tenant_owned_row_level_security`          | re-render the assets from the classification so `nodes`, `node_registrations` and `node_registration_history` get their forced policies (#265 added these per-app migrations after #349's)                                                                                                                                           |
| the migration #265 uses for each app's foreign keys to `tenants`                                            | the `(tenant, node)` foreign keys of `runs`, `generation_requests` and `events` to `nodes` ride with that app's Tenant root keys, because they also cross apps                                                                                                                                                                       |

`server/src/database/classification.rs` gains `table("identity", "node_names", DeploymentWide)` with its reason, `table("identity", "nodes", TenantOwned)`, `table("federation", "node_registrations", TenantOwned)` and `table("federation", "node_registration_history", TenantOwned)`, and loses the three removed tables. No table is `Pending(241)` afterwards, which meets #265's criterion. RLS stays Tenant-granular. No policy mentions a Node (ADR 0040).

`server/migrations/README.md` gets one paragraph per changed migration, and the policy functions and assets list (`README.md:393`) stays accurate. No new raw SQL is introduced.

## 8. Test plan

| #241 criterion                                                                                                                                                       | Tests                                                                                                                                                                                                                                                                                                                                          |
| -------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| A Node's address is `aidash://<deployment>/<Node name>`; `/.well-known/aidash` publishes the Deployment identity and key and no Nodes                                | `crates/aidash-domain/src/nodes/tests.rs`: parse, display, refusal of a Tenant segment, `MAX_LEN`. `server/src/apps/federation/peer/tests/well_known.rs`: body with a key file, body with `key: null`, no `capabilities` or `clusters` field                                                                                                   |
| Across Deployments, a receiving Node refuses an unregistered sending Node and a token signed by a key other than the pinned one                                      | rewritten `pair` in `server/src/apps/federation/transactions/tests/protocol_acceptance.rs:131-176`: unregistered sender gets 401; a second key file on the sender gets 401 with the same body; after re-registration with the new key ID the request passes                                                                                    |
| Inside one Deployment the same acceptance code refuses cross-Tenant exchange without a Node Registration and decides exchange within a Tenant by the Tenant's policy | new `server/src/apps/federation/remote/tests/node_exchange.rs`, in-process through `LocalTransport`: two Tenants in one database, discovery from Node A to Node B of another Tenant refused until B registers A and maps A's subject; two Nodes of one Tenant pass with the initial policy and fail after `node.use` is removed from `members` |
| Turning the switch off refuses Federation with other Deployments; a Self-hosted Deployment without a key file refuses it and still federates inside                  | `protocol_acceptance.rs` cases with `federation = false` on the receiver (401) and on the sender (`federation_disabled`); a `node_exchange.rs` case whose settings name no `[deployment.key]` and whose in-process exchange still passes                                                                                                       |
| Creating a Tenant creates its default Node; creating a Node beyond "Nodes per Tenant" or the Tier's limit is refused                                                 | new `server/src/apps/identity/tests/nodes.rs`: `tenants::create`, Personal Tenant provisioning and every fixture helper leave one `nodes` row with `is_default`; the eleventh `POST /nodes` under the default rule answers 403 `node_limit_reached`; a fake `TierLimits` returning 1 refuses the second                                        |
| Requests at protocol version 0.2 or 0.3 are refused                                                                                                                  | `server/src/apps/execution/tests/http_protection.rs`, which already sends `x-aidash-protocol`, gains cases where `0.2` and `0.3` answer 400 before any token is read                                                                                                                                                                           |

Further tests the design adds:

- `crates/aidash-capability/src/federation/tests.rs` covers mint and verify, each claim check in order, a body and a path mismatch, a 61 second lifetime, and an unknown `kid`.
- `server/src/apps/identity/tests/nodes.rs` also checks that a deleted Tenant's Node refuses inbound exchange and a restore admits it again.
- `server/src/apps/federation/peer/tests/peer_authorization.rs`, which holds today's Peer Mapping route tests, gains the same source subject mapped at two receiving Nodes, and a conflict within one Node that answers the neutral 409.
- `every_table_is_classified_and_matches_its_policies` (`server/src/apps/execution/tests/tenancy.rs:84`) passes with the new tables and no `Pending(241)`.
- The existing local and foreign generation tests pass with `local_task_id` written by the application, and a new case checks that a foreign request leaves both `local_*` columns null.
- A catalog test approves an agent on Node A, checks that its subject is keyed by A's address, and checks that a Run in a Workspace on Node B refuses that agent until it is approved on B.

**Fixtures.** `fixtures.rs::pair` (`server/src/apps/federation/transactions/tests/fixtures.rs:85-153`) and `protocol_acceptance::pair` start each Deployment with `[deployment] id`, `endpoint` and a `[deployment.key] file` generated into the test's temporary directory by `manage deployment-key generate`, create one Tenant per Deployment through `tenants::create`, read each other's key ID from `/.well-known/aidash`, and register each other's default Node in both directions through `POST /api/tenants/{tenant}/nodes/{node}/registrations`, then create Peer Mappings by Node. `register_peer` and the trust `POST` (`fixtures.rs:102-125`) go. `scoped_peer` (`server/src/apps/identity/tests/scoped_remote_execution.rs:6523`) replaces its raw `peers` inserts (`:416-454`) with registration rows through the same route. `support/peer.rs` (`server/src/apps/execution/tests/support/peer.rs:33-53`) overrides `deployment` instead of `node_id` and exposes the fixture Tenant's default Node. The files that register peers with a builder plus `credential_env` (`server/src/apps/federation/peer/tests/registration.rs`, `journals.rs`, `peer_authorization.rs` and `peer_execution.rs` in that directory, and `server/src/apps/federation/remote/tests/discovery_fixtures.rs`) and the two that insert `peers` by raw SQL (`server/src/apps/identity/tests/peer_graph_review.rs:89` and `server/src/apps/identity/tests/dashboard_oidc/tenant_authority.rs:434`) switch to the registration route. The federation partition (`scripts/rust-test-partitions.py --partition federation`) lists the ten targets to run after each step of section 9.

## 9. Work breakdown

Each commit compiles, passes its named check, and leaves the tree in a state the next commit builds on. Intermediate breakage is confined to the step that declares it.

1. **Deployment id types and settings.** Add `crates/aidash-domain/src/nodes.rs` with `DeploymentId`, `NodeName`, `NodeAddress`, `DeploymentKeyId`. Replace `validate_node_id` with `DeploymentId::parse`. Move `[node]` into `[deployment]` and rename `Config`, `Store` and `Registry` fields and the legacy env. Rename `registry_node` to `registry_deployment` with the SQL checks. No Node semantics yet. The checks are `cargo clippy -D warnings` on the workspace, `cargo test -p aidash-domain`, the foundation and server-unit partitions, and `makemigrations --check`.
2. **`nodes`, `node_names`, default Node, `node.create`.** Tables, models, classification, rendered policies, `TenancyRules.nodes_per_tenant`, `TierLimits` port, `create_default` in `tenants::create` and the three fixtures, the six actions, the Node routes, administration events. The checks are `makemigrations --check`, the `migrations.rs` counts, `tenancy.rs:84`, the new `nodes.rs` tests, and the identity partition.
3. **Node-owned data.** `authorization_workspaces.node`, `WorkspaceInput.node` with `node.use`, `runs.node`, `events.node`, `generation_requests.node` with the `local_*` rewrite, `authorization_catalog.node` with routes, `qualified_agent` on a `NodeAddress`, the `home_node` content change in every column of section 2, `rules.rs` budget. The checks are the execution, knowledge-marketplace and collaboration partitions, the catalog tests, and the Bruno contracts regenerated and passing `--check`.
4. **Deployment Key.** `DeploymentKey` port, `FileDeploymentKey`, `manage deployment-key`, startup refusal rules, `DeploymentIdentity` on `/.well-known/aidash`, `[deployment] federation`. The checks are unit tests for loading and refusal, `manage deployment-key generate` then `show` in a temporary directory, and the well-known tests.
5. **Federation token.** The `jws` generalization in `aidash-capability`, `FederationClaims`, mint and verify. The check is `cargo test -p aidash-capability`, with the existing capability-token tests unchanged.
6. **Node Registrations and Peer Mappings.** `node_registrations` and history replacing `peers`, the registration routes with the well-known fetch, Peer Mapping re-keying and routes, `authorization_peer_graph_grants` re-keying, `authorization_remote_admissions` and `authorization_remote_grants` keys, removal of `peers`, `peer_events`, `atomic_peer_trust`, the Deployment Administrator routes and the dashboard peer, mesh and trust UI. Protocol `0.4`. This step breaks the federation tests until step 8 and says so in its message. The checks are `makemigrations --check`, `migrations.rs`, `tenancy.rs:84`, the identity partition, `npx tsc -b` on the touched web files, and the Bruno contracts.
7. **Acceptance and transports.** `accept`, `accept_grant_viewer`, the boundary adapter, `Admitted` replacing `peer_node()` in 19 files, `LocalTransport`, `HttpTransport` minting, `Federation::request` routing, `federation_disabled`. The checks are the `node_exchange.rs` in-process tests, the boundary tests for 0.2 and 0.3, and the capability and generation peer routes compiling against `Admitted`.
8. **Two-Deployment fixtures and tests.** Rewrite both `pair`s, `scoped_peer`, `support/peer.rs` and the seven peer-registering test files; delete the peer secrets from the scripts. The check is the full federation partition, with `scoped_remote_execution`, `transaction_protocol` and `transaction_acceptance` run directly from the test binaries on macOS.
9. **Dashboard.** Node selector, Workspace form, Node and Node Registration pages, Peer Mapping Node field, orval regeneration. The checks are `npx tsc -b` after regenerating `web/src/generated/`, the golden-path browser spec, and a Playwright spec that creates a Workspace in a second Node.
10. **Docs.** `docs/protocol.md` Trust and identity, `docs/authorization.md` actions and inbound sections, `docs/transactions.md` authority, `server/migrations/README.md`, `server/settings/base.example.toml`. The checks are `trunk fmt --ignore-git-state` on the touched files and a grep that no doc names `credential_env`, `x-aidash-node` or `[node]`.

## 10. Open points

The coordinator's review of 2026-10-19 settled each item as recommended, except item 3, which was removed from the original scope. The items stay here as the record of the alternatives. The 2026-10-23 completion decisions supersede items 3 and 4; Issue #301 settles item 5, whose implementation remains tracked separately.

1. **Transaction trust.** `atomic_peer_trust` and `GET`/`POST /api/transactions/trust` were parked "until #241" (#359) and no decision replaces them. The recommendation, assumed above, is to fold trust into `node_registrations.atomic_transactions`, set under `node_registration.update`, and to require it at the coordinator for each participant and at each participant for the coordinator, in place of `transactions/admission.rs:61-68` and `authority/persistence.rs:185-195`. The trade-off is that a Tenant decides who may hold reservations against its Node, which is the #363 direction, at the cost of moving one more Deployment Administrator authority. The alternative keeps a Deployment-wide trust table keyed by Node address and a Deployment Administrator route.
2. **Where the Federation switch lives.** The design assumes `[deployment] federation`, a setting changed with a restart, like the Tenancy Rules. The alternative is a Deployment-wide row and a Deployment Administrator route that flips it live. The route is faster to operate on Cloud and adds a table, a route, a UI control and a classification entry.
3. **Disabling a Node.** The original review kept the scope to creation and deferred stopping to #388. The completion design and ADR 0083 now settle recoverable deletion, default switching, restore and purge, without a separate disabled Node state. Section 5.6 distinguishes current controls from pending physical purge integration.
4. **Key rotation.** The original recommendation accepted manual re-registration after every rotation and deferred a published `keys[]` set. The completion design and ADR 0084 supersede that limit with predecessor-proven rollover: publish now, activate after 24 hours and retire the predecessor seven days later. Sections 1.6 and 4.4 describe the implemented schedule, receiving keysets and independent manual confirmation after missed overlap or compromise; public discovery alone never advances trust.
5. **Remote Deployment Administrator Graph View.** The original design retained the ADR 0064 grant pending #301. [Issue #301](https://github.com/kent8192/aidash/issues/301) now records removal of Peer Graph Grants and the special administrator projection paths, using ordinary Subject federation and receiving Tenant policy instead. Future implementation is tracked by #486; the current server and UI still retain the grant and its key controls. This document records the decision without claiming that removal or the other #301 runtime changes are complete.
6. **Replay within 60 seconds.** The design assumes no `jti` store, a token bound to one request, and handlers idempotent per source. The alternative is a `federation_nonces` table with a 60 second retention. The table costs a write per request and a cleanup loop, and the Capability Token has lived without one.
7. **Catalog Approvals per Node.** A new Node starts without approvals, so every agent, Sandboxed tool, Research provider and Decider that needs one is approved again. The recommendation is no copy affordance in this PR, and a later "copy approvals from Node" action if it hurts. The decision "Catalog Approvals name a Node" stands.
8. **The Free Tier's single Node.** The design assumes that this PR adds the `TierLimits` port with a Community default of no Tier limit, and that #242 supplies the Enterprise implementation. The alternative is to hardcode 1 for Personal Tenants here, which duplicates a rule #242 owns.
9. **The sending Node for dashboard work.** The design assumes that the Node selected in the dashboard shell is the sending Node for discovery and Graph View. The alternative is a per-action Node choice. The selector is one control and matches the #241 decision "Choosing a Node", which names a Node selector inside the Tenant.
10. **Column renames.** The design assumes that columns that hold a possibly-remote Node keep their names (`home_node`, `source_node`, `node_id`, `coordinator`, `peer_node`, `dispatcher_node`) and change content, and that only local Node names and Deployment ids are renamed. The alternative is to rename every `node_id` to `node_address`, and the rename touches about 2,000 references for no behavioral gain.
11. **Clock skew between Deployments.** Settled by the coordinator on 2026-10-20. `FederationClaims::verify` accepts `iat` up to `IAT_LEEWAY_SECS` (30 seconds) in the future, so a sender 20 seconds ahead is accepted and one 40 seconds ahead is refused. It still requires `exp - iat <= 60` and `exp > now`.

## 11. Corrections found while implementing steps 2 and 3

- A Workspace's Node is `workspaces.node`, not `authorization_workspaces.node`. The Workspace entity is a row of `workspaces`, and `runs.node`, `events.node` and `generation_requests.node` copy it with a subselect, so no join is needed. `Access::workspace` reads it and sets the Access's Node.
- `events.node` is nullable. A Tenant-level event, such as a Marketplace announcement, has no Workspace and so no Node. `events.tenant` is NOT NULL, because step 6 removed `peer.registered`, the only Tenant-less event. The CloudEvent `source` is the Node address, or the Deployment id for a Tenant-level event, and `Event::cloud_event` takes the Deployment.
- `generation_requests_home_task` keeps its status predicate and becomes `(home_node, task_id)` unique where `local_task_id IS NULL AND status IN (...)`. A plain unique constraint would forbid a second foreign request for a Task whose first one ended. `generation_requests_local_task` is unique on `local_task_id`.
- A Run is local when `home_node` is the address of its own `runs.node` in this Deployment (`Run::is_local`). `Run`, `RunMetadata` and `Request` carry `node`. A foreign generation request and a Run a peer's Home admitted execute on the mapped Tenant's default Node until step 6 gives admissions a receiving Node.
- Catalog Approvals are read per Node through the Access's Node: the Workspace's or Run's once an operation has named it, the Tenant's default Node otherwise. Marketplace activation, Generation policy dependency checks, the Graph View, Workbench permissions and the research provider listing use the default Node. The approvals routes take `node` as the body field or the required query parameter.
- `node_registration.read` and `node_registration.update` are not added by step 2. They arrive with their routes in step 6.
- Participants of a transaction manifest, the node strings of peer requests and peer Delegations still name Deployments or whatever string the peer sent. Peer flows call `agent_subject(text, ...)` with that string, so a peer-executed agent authorizes only once step 6 and 7 name the receiving Node. `NodeName::derived` and `NodeAddress::derived` give fixtures a stable Node name for a Tenant name.
- Peer file transfer names the recipient by Node address. `recipient.node_id` and `target.node_id` are Node addresses, the sender finds the peer through `deployment_of(node)`, and the receiver checks the Deployment part, finds the recipient Area by its exact home Node address and builds the agent subject from that address. `Federation::peer` resolves by the Deployment part until Node Registrations replace `peers` in step 6.

## 12. Corrections found while porting the remaining paths

- A usage reservation names two things by Deployment. The dispatcher and the provider's `node_id` are the Deployment of the registry that holds the provider (`Usage::validate` requires them equal), and the Peer compares them with `deployment_of` of the Node address it authenticated. The Run, the grant, the admission and the intent name Nodes by address.
- A generation intent's `home_node` is the address of the Node the Task's Workspace belongs to. The Home sends `prepare` and `cancel` as that Node, and a peer's `describe` and `cancel` are accepted only at the Node the intent names. The lineage of an intent still names the Deployment, because the accounting rows do.
- A callback that a remote Run or a grant sends comes from the Node the Run executes at or the grant was issued from: `Store::remote_node` finds it from the Run, the admission or the grant, and `usage_sender` returns it. The Home port takes the Workspace a grant belongs to and sends as that Workspace's Node.
- A Workspace's native memory banks are bound at `<deployment>/<workspace node>`, and a generated Agent's origin is a Node address. The providers a binding pins keep the receiver's Deployment.
- A remote Run's input keys are `<executing Node address>:<task>:<key>`, on both sides.
- Discovery names the agents it finds for the sending Node by that Node's address, and its Catalog Approvals decide them. `GET /api/registry/versions` takes the Node as a required `node` query.
- Transactions still send from the submitting Tenant's default Node when no Access names one. One transaction naming more than one Node of a Deployment stays refused by `Manifest::validate`.
