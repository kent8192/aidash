# Credential Broker

Community, Apache-2.0. `aidash-capability` contains portable Capability Token
contracts; `aidash-broker` is a persistence-free Cloud Run binary. Production
composition uses `aidash-integrations::capability` for minimal GCP REST calls.
There is no self-hosted broker mode and no GCP SDK dependency. Self-hosted
workers continue to use environment credentials directly.

Issue [#158](https://github.com/kent8192/aidash/issues/158) owns provider OAuth
acquisition and retirement of the model-provider Env path under ADR 0006.
This change preserves that existing path until the cutover. Broker and Cloud
`ProviderAccess` behavior only reads Key Material already stored by the internal
application write use cases; it does not expose pasted-key create/rotate APIs.

## Admission and authority

The worker mints a compact EdDSA JWS after the operation's existing policy
decision, using the pinned Cloud KMS `EC_SIGN_ED25519` version. KMS receives the
raw JWS signing input in `data`, with CRC32C integrity verification. Private
signing keys never leave KMS. Workers log the `jti` and Token Subject.

The broker verifies injected public keys indexed by `kid`, issuer, its own
environment audience, a maximum 60-second lifetime, numeric pinned secret
version, operations, and the model. Expiry is checked only at admission; a
stream may continue until the inference deadline. Tokens are replayable until
expiry. There is no single-use or cumulative-spend state.

Tenant ownership of a Provider Credential, its active state, and its current
pinned version are checked by Cloud `ProviderAccess` **before minting**. Runs
use the Provider Credential ID pinned at admission; maintenance uses the
mapped local Tenant's current Provider Credential Binding. The signed claims
are the broker's authority, so it has no database or credential metadata lookup.
A maintenance Token Subject's Tenant must match the
claim's Tenant. Ownership rejection tests belong at the mint boundary; broker
rejection tests check the subject/claim match, malformed credential/version,
and unavailable secret versions. An unknown Provider Catalog ID returns
`403 capability_tenant`.

Run-less Token Subjects record the Maintenance purpose as exactly one of
`memory_indexing`, `memory_retention`, `memory_reflection` or `memory_retrieval`.
The call kind is expressed separately by `ops` (`embeddings` or `chat`), so a
purpose does not grant either operation implicitly. The obsolete
`memory_embedding` and `memory_model` subjects are rejected.

The broker derives the secret resource exclusively from its configured BYOK
project number, environment prefix and the signed Provider Credential UUID.
It never accepts a secret path, upstream base URL or tenant override from an
HTTP request. Provider Catalog v1 fixes OpenRouter to
`https://openrouter.ai/api/v1`; tests replace this privately with a loopback
provider. Redirects are disabled, and inbound `Host`, forwarding, authorization
and other headers are never copied upstream.

## HTTP contract

The endpoint returned to workers ends with `/api/v1`; integrations append
their existing paths unchanged.

| Operation  | Method and broker path                         | Success response limit |
| ---------- | ---------------------------------------------- | ---------------------- |
| chat       | `POST /api/v1/chat/completions`                | 1 MiB                  |
| discovery  | `GET /api/v1/models/{author}/{slug}/endpoints` | 2 MiB                  |
| discovery  | `GET /api/v1/endpoints/zdr`                    | 8 MiB                  |
| embeddings | `POST /api/v1/embeddings`                      | 1 MiB                  |

Chat bodies must name the exact model, reject the `models` fallback field,
include positive `max_tokens` no greater
than `max_output_tokens`, and set `provider.zdr: true`. OpenRouter embeddings
must name the exact model and set ZDR. Model discovery paths must match the
claim; ZDR discovery is an operation-wide catalog. Chat requests are limited to
16 MiB so the application's 8 MiB raw-media allowance fits after base64 encoding
and JSON framing. Other requests remain limited to 1 MiB. Query strings,
absolute URIs, percent-encoded paths, empty segments and
path traversal are rejected. Violations are rejected, never rewritten.
OpenRouter's [model fallback list](https://openrouter.ai/docs/guides/routing/model-fallbacks)
would add authority outside the single signed model, so even empty, null or
same-model fallback lists are rejected before Key Material lookup.

Capability failures use `401` for signature, key, expiry or audience failures,
and `403` for tenant, credential, operation, model or claim violations:
`{"error":{"code":"capability_<reason>"}}`. Workers must treat these as
non-retryable configuration/mint errors, including expiry; never re-mint to
retry. Unavailable pinned Key Material returns `403 capability_credential`.
Provider statuses are preserved while error messages are sanitized using the
same fixed reasons as `safe_upstream_reason`, with at most 16 KiB read.
Redirect `Location` headers are discarded.

Streams relay chunks before upstream completion without accumulating the body.
Their total-byte limit equals the non-streaming limit. The total inference
deadline includes request reads, Key Material lookup, upstream admission and
response reads. It is at most 3600 seconds. Disconnects and truncated streams
are audited. Audit entries contain `jti`, subject, Tenant, Provider Credential,
version, provider, model, operation, status, latency and numeric token usage;
SSE usage is read from final chunks. Bodies, tokens, arbitrary usage fields and
Key Material never enter logging. v1 does not write audits back to Aidash.

The in-memory cache uses `(secret, version)` and a 60-second TTL, with
`SecretString` holding Key Material. The per-instance Provider Credential token
bucket defaults to 5 requests/second and a burst of 20. The minimum burst is 3:
one media inference makes two concurrent discovery requests and one chat call.
Runtime and Terraform reject smaller bursts. Cache and rate-limit
bookkeeping are bounded to 10,000 entries. Cross-instance rates are approximate.

## Deployment and verification

Build from the repository root:

```sh
docker build -f crates/aidash-broker/Dockerfile -t aidash-broker .
cargo test --locked -p aidash-capability -p aidash-broker
cargo test --locked -p aidash-integrations capability::tests
python3 -m unittest discover -s infra/gcp/tests -p test_broker_terraform.py -v
bash infra/gcp/check.sh
```

Human-run bootstrap receives `byok_broker_environments`, a set of retained
environment IDs. It creates `aidash-<environment_id>-broker` service accounts
in the shared application project and outputs `broker_service_accounts`, a map
from environment ID to email. It also owns the permanent per-environment KMS
key ring and Ed25519 signing key and exports their IDs in `broker_signing_keys`.
Both resources have Terraform destruction protection. Keep this bootstrap set
when disabling the broker or retiring its VM: automation does not own key lifetime.
Bootstrap alone grants these accounts the BYOK
custom role `aidashByokBrokerRead` (`secretmanager.versions.access`,
`secretmanager.versions.get`, `secretmanager.secrets.get`), conditioned on
`projects/<project-number>/secrets/aidash-<environment_id>-cred-`. Deploy automation
cannot grant BYOK read access. Secret Manager IAM Conditions use project
numbers, as specified by the
[official resource-name reference](https://cloud.google.com/iam/docs/conditions-resource-attributes).

`infra/gcp/modules/credential-broker` consumes #136's `byok_project_id`,
`secret_prefix`, bootstrap-created `broker_service_account_email` and stable
`signing_key_id`. It creates no service account, signing key or BYOK-project IAM.
In the environment
controller's `credential_brokers[environment_id]` configuration, set
`broker_service_account_email` and `signing_key_id` to the corresponding
bootstrap map entries before
enabling the broker. Cloud Run uses that account; deploy automation gets
`roles/iam.serviceAccountUser` on it in the application project. The VM runtime
holds only `roles/cloudkms.signer` on the signing key and no broker role. Public
keys come from Terraform's KMS version data sources; the broker never calls KMS
at runtime. Module tests prove it cannot create a second BYOK read grant;
bootstrap tests own the proof that its binding is the only `versions.access`
grant in the BYOK project.

Managed `credential_brokers[environment_id]` also accepts `signing_version`
(default `"1"`) and `verification_versions` (default `["1"]`). After a human
operator creates and enables a replacement KMS version, set, for example,
`signing_version = "2"` and `verification_versions = ["1", "2"]`. The worker
descriptor selects version 2 while the broker verifies both versions. Keep the
old public key until all capabilities signed with it expire, then remove it from
the verification set. The stable bootstrap `signing_key_id` stays unchanged.

Mock lifecycle plans prove disable, re-enable and environment removal change
only the service/signing grant; bootstrap key and key-ring plans remain no-ops.
Automation also refuses any plan that would delete a KMS key, key ring or version.
Broker changes that remove or replace a live service drain all affected workers
before any Terraform apply, including applies triggered by another interrupted VM.

### Operators who applied a pre-merge draft

This change has never been applied to a Cloud environment. If an operator applied
an earlier draft that kept keys in the environments state, pause automation and
back up both remote Terraform states before switching ownership. Keep the same
key IDs and `broker_signing_region`; Cloud KMS [key names cannot be reused after
deletion](https://docs.cloud.google.com/kms/docs/resource-hierarchy).
For each existing environment (shown as `test`), import the existing resources
into the initialized bootstrap state, then remove only their old state addresses:

```sh
terraform -chdir=infra/gcp/bootstrap import 'google_kms_key_ring.capability["test"]' 'projects/PROJECT/locations/us-central1/keyRings/aidash-test-capability'
terraform -chdir=infra/gcp/bootstrap import 'google_kms_crypto_key.capability["test"]' 'projects/PROJECT/locations/us-central1/keyRings/aidash-test-capability/cryptoKeys/capability'
terraform -chdir=infra/gcp/environments state rm 'module.credential_broker["test"].google_kms_crypto_key.capability[0]'
terraform -chdir=infra/gcp/environments state rm 'module.credential_broker["test"].google_kms_key_ring.capability[0]'
```

Use the imported `broker_signing_keys` output in `credential_brokers`. Review both
plans and require no key/key-version destruction or replacement before any apply
or automation resume. These are operator-only state/import steps; no migration
automation or Cloud mutation runs as part of this implementation.

### Runtime composition

The deploy pipeline is an accepted trust root (ADR 0005). Its shared-project
`roles/iam.serviceAccountAdmin` and ability to redeploy broker code mean the
bootstrap split alone cannot protect Key Material from a compromised pipeline.
IAM deny policy and GitHub/WIF entry-point hardening are tracked in
[#151](https://github.com/kent8192/aidash/issues/151); sandbox verification is
[#152](https://github.com/kent8192/aidash/issues/152). These follow-ups are
separate from this implementation. No Cloud infrastructure has been applied.

Workers configure `[provider_credentials.broker]` with the module's
`worker_configuration` (`endpoint`, `issuer`, `audience`, `kid`). Its audience
must equal the Store's environment. Managed environments put the non-secret
Store/broker descriptor in VM metadata. Host bootstrap writes it into a
read-only mounted settings directory and sets `AIDASH_PROVIDER_CREDENTIAL_SETTINGS`;
Reinhardt composes that source above TOML defaults. Fingerprints remain runtime
Secret references. Enablement, key-version changes and disablement wait for active
work to drain before reloading application settings, without restarting VM power.
Startup composes `MetadataTokenSource`,
`KmsTokenSigner` and `CapabilityIssuer` in the worker process. `TenantAccess`
reloads active metadata and the current numeric pin on every call; it uses the
Run's admitted ID or the current local Maintenance binding. Inference adapters
attach the exact model, operation and approved output bound immediately before
resolution. BYOK model deadlines above 3600 seconds are rejected at the shared
Registry registration/import/Marketplace validation boundary and before use.
BYOK model IDs use catalog path segments with a maximum of 256 bytes, checked
by the same domain rule at Registry admission and capability minting.
Env-based deadlines and per-call direct key resolution retain their behavior.

Cloud Run uses internal ingress with its invoker IAM check disabled and default
egress. Production defaults to 1 minimum instance; other environments use 0.
Maximum instances default to 10 and timeout to 3600 seconds. These are module
inputs. Each instance explicitly uses 1 GiB of memory, one CPU and a maximum
of four concurrent requests to bound buffered media and JSON parsing copies.
Bootstrap rejects overlapping credential prefixes such as `prod` and
`prod-cred-blue`; environment composition also checks managed environments that
have no broker. The existing nonproduction environments root remains nonproduction;
its opt-in `credential_brokers` configuration is forwarded by the controller.
The module can also be composed by a production root. PR previews cannot enable
a broker. Apply the human-run bootstrap with #136's BYOK roles and #137's
broker identities and read bindings before enabling it.

The module returns non-secret `worker_configuration` with endpoint, issuer,
audience and KMS `kid`. The broker receives numeric project number, environment
prefix, public keys and rate/timeout settings as environment variables. No
provider Key Material or private signing key is deployed in environment config.

After #158 supplies provider OAuth acquisition, an optional live OpenRouter
smoke test is separate evidence: in an enabled staging environment, obtain a
Provider Credential through Provider Authorization, bind it to an approved
model, execute a small authorized Run, and
check worker/broker audits share `jti` with successful status and token usage.
Rotate through Provider Authorization and verify the next call names the new
pinned version, then revoke it and verify admission fails. Do not print tokens,
Key Material or request bodies. A live test is not required for deterministic
fake-provider acceptance.
