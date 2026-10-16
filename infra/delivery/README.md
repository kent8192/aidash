# Delivery setup automation

This directory automates the [eight-step activation checklist](../../docs/operations/release-deployment.md#activation-checklist).
Use the existing task checkout. Terraform owns project creation, bootstrap IAM,
private buckets, protected GitHub Environments, workflow variables, additive
required checks, verified Email Routing aliases, and production backup/monitoring
resources. Python handles read-only adoption, reviewed plans, App registration,
private secret handoff and certificate renewal. No command merges a release PR
or approves a production deployment.

| Checklist | Automated work                                                                                                              | Manual input or acceptance still needed                                                                     |
| --------- | --------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- |
| 1         | Four disabled workflow flags; existing CI validates the delivery tools                                                      | Review/merge this implementation into main and the default develop branch; synchronize their history        |
| 2         | Five Environments, exact branch policies, narrow App-bound checks; manifest/key handoff                                     | Review GitHub App Create/Install consent, select only Aidash; human production reviewer                     |
| 3         | Dedicated project on the same billing account, isolated bootstrap, public config composition, protected secret registration | Explicit GCIP pools and production runtime/IdP credentials; live IAM denial checks                          |
| 4         | Secret metadata, dedicated renewal WIF/SA, scheduled DNS-01 issuance/renewal, running edge refresh                          | Scoped Cloudflare DNS token, ACME contact; configure external OAuth/SSO client callbacks and verify sign-in |
| 5         | Existing Sending/destination discovery, three alias rules, optional guarded Routing DNS, optional Sending token handoff     | Issue scoped bootstrap tokens, verify private destination, prove actual inbox arrival                       |
| 6         | Exact-CI staging/release flow and explicit PR preview lifecycle                                                             | Supervised authentication/Host/CSRF/lifecycle/mail acceptance, then explicit activation                     |
| 7         | Prepare GKE backup agent; after cluster exists, daily backup and four monitoring policies; read-only inspection             | Recovery target/isolation and restore drill, Home-ledger retention, RPO/RTO acceptance                      |
| 8         | Exact main CI and staged digest promotion, protected production gate and fixed mail                                         | Human release merge/Environment approval and actual production acceptance                                   |

## Private inputs and remote state

Use Terraform 1.14.8+, Python 3, OpenSSL, authenticated `gh` and `gcloud`.
The setup helper obtains short-lived CLI credentials in process memory; they are
never shell arguments, Terraform variables or displayed command output.
Do not enable Terraform debug logging. Work from the repository root:

```sh
umask 077
mkdir -p .ignore/delivery/operator
python3 infra/delivery/control/setup.py inventory
python3 infra/delivery/control/setup.py verify
```

Save your inputs as private JSON (`*.tfvars.json` is a valid Terraform input
format). Copy [account.example.tfvars.json](account.example.tfvars.json) and
[secrets.example.tfvars.json](secrets.example.tfvars.json) into the ignored private input
directory and fill them locally. Their permissions must be 0600. Secret values
are accepted only for fixed protected destinations and are passed to `gh` stdin;
Terraform never manages GitHub secret payloads. Empty placeholder secrets are
refused: remove entries that are not being installed in that operation.

Every stack uses an explicit private GCS state bucket. The helper keeps a private
module copy, input, log, saved plan and outputs under `.ignore/delivery/operator/<stack>`.
Its public plan summary contains resource addresses/actions only. Review that
summary and the private saved plan before applying. Deletions and replacements
are refused by default. Replan after a partial provider failure; do not reuse a
stale saved plan or erase the state.

```sh
python3 infra/delivery/control/setup.py plan --stack github --file .ignore/delivery/operator/github.tfvars.json --state-bucket YOUR_PRIVATE_STATE_BUCKET
python3 infra/delivery/control/setup.py apply --stack github --state-bucket YOUR_PRIVATE_STATE_BUCKET
```

For the initial GitHub phase, `github.tfvars.json` contains the current adoption
inventory plus `production_reviewer_user_ids`; `configure_variables` defaults
false. This preserves unrelated rulesets/classic protections and existing account
variables. After account setup, retain the full account inputs when replanning;
do not switch back to protection-only. See [GitHub controls](github/README.md).

## GCP bootstrap and isolation

Use [project inputs](projects/projects.example.tfvars), then the two
[bootstrap examples](gcp/README.md) with `--stack projects`, `nonproduction` and
`production`. Keep existing nonproduction state at its actual prefix; the current
installation uses `--prefix terraform/bootstrap`. Root bootstrap does not create
application clusters or enable deployment. GCIP and BYOK are explicit bootstrap
inputs; their application pools/runtime values are separate inputs.

A first production bootstrap may use the existing human-controlled state bucket
to create its own private bucket. Immediately relocate that bootstrap state:

```sh
python3 infra/delivery/control/setup.py relocate-production-state --stack production --state-bucket INITIAL_STATE_BUCKET --target-state-bucket PRODUCTION_STATE_BUCKET
```

This checks the production output, refuses an existing target state, preserves
lineage/resources/outputs, and retires only the copied source object's current
exact generation. Private versioned history remains retained. Every later
production plan/apply must use `PRODUCTION_STATE_BUCKET` with the same prefix.
Application Terraform/lifecycle state also uses that isolated production bucket.
Project-creation and GitHub/Cloudflare control state use their separate fixed
`terraform/delivery/<stack>` prefixes; do not give runtime Pods state access.

`--review-bootstrap-migration` is only for the inspected legacy nonproduction
bootstrap: it retires this project's own obsolete VM/IAP grants and narrows its
own WIF bindings. IAP/OS Login APIs remain enabled. Other deletes/replacements
remain refused. For an interrupted **new** project's default-network cleanup,
`repair-project --stack projects --file ...` checks the recorded tainted project,
Aidash labels, correct billing, absence of instances and unmodified default
network before cleanup. It cannot retire a customized or running project.

After successful bootstrap and [App setup](app-README.md), fill the account input
with the two private `outputs.json` paths, the App metadata path, both public GCIP
pool maps, zone/account IDs and ACME email. Compose and review the account plan:

```sh
python3 infra/delivery/control/setup.py compose --file .ignore/delivery/operator/account.tfvars.json --output .ignore/delivery/operator/github.tfvars.json
python3 infra/delivery/control/setup.py plan --stack github --file .ignore/delivery/operator/github.tfvars.json --state-bucket YOUR_PRIVATE_STATE_BUCKET
python3 infra/delivery/control/setup.py apply --stack github --state-bucket YOUR_PRIVATE_STATE_BUCKET
python3 infra/delivery/control/setup.py secrets --file .ignore/delivery/operator/secrets.tfvars.json
```

Composition publishes only controller fields; it strips bootstrap-only registry,
node identity and signing-key outputs. `gcip_tenants` contains public pool/IdP
metadata only. Client secrets belong in the corresponding protected IdP secret.
Every bootstrap broker in `byok_broker_environments` becomes a
`credential_brokers` entry built from its bootstrap service account, signing key
and Secret prefix; an omitted broker would be drained and removed on the next
reconcile. Each entry is disabled unless the optional per-tier
`credential_brokers` account input sets `enabled`, `signing_version` or
`verification_versions` for a bootstrap-created broker ID.
Composition leaves every workflow disabled, the staging-required flag off and
all activation acknowledgements false. Complete account setup before activating.

## Cloudflare inputs and apply

The existing `aidash.run` Email Sending registration reports Configured. Do not
onboard it again. Follow [token issuance permissions](cloudflare/README.md#tokens-to-issue-later)
and copy [cloudflare.example.tfvars.json](cloudflare.example.tfvars.json) into
`.ignore/delivery/operator/cloudflare.bootstrap.tfvars.json`. Fill the bootstrap
token, exact account ID and already verified private forwarding destination.
The discovery command makes only GETs and resolves existing registration and
destination IDs; it never sends a verification email or notification.

```sh
python3 infra/delivery/control/cloudflare_setup.py discover --file .ignore/delivery/operator/cloudflare.bootstrap.tfvars.json --output .ignore/delivery/operator/cloudflare.tfvars.json
python3 infra/delivery/control/setup.py plan --stack cloudflare --file .ignore/delivery/operator/cloudflare.tfvars.json --state-bucket YOUR_PRIVATE_STATE_BUCKET
python3 infra/delivery/control/setup.py apply --stack cloudflare --state-bucket YOUR_PRIVATE_STATE_BUCKET
```

The bootstrap token variable is ephemeral and is supplied again in process
memory at saved-plan apply. The private forwarding destination does appear in
Cloudflare state, so restrict access to that backend. The Sending token and
certificate DNS token are separate protected GitHub secrets. A manually issued
Sending token can be installed with `setup.py secrets`. If using the optional
account-token provisioner, first review its dry run, then pass `--apply`; it checks
live Environment protection before creation and handoff, never writes token values
to Terraform, and revokes a newly created token if handoff fails:

```sh
python3 infra/delivery/control/cloudflare_setup.py sending-token --file .ignore/delivery/operator/cloudflare.bootstrap.tfvars.json --output .ignore/delivery/operator/sending-token-receipt.json
python3 infra/delivery/control/cloudflare_setup.py sending-token --file .ignore/delivery/operator/cloudflare.bootstrap.tfvars.json --output .ignore/delivery/operator/sending-token-receipt.json --apply
```

Keep `configure_routing_dns=false` for already-ready incoming Routing. Opting in
checks the complete existing apex MX/SPF against Cloudflare's proposal and refuses
another mail provider or duplicate/conflicting SPF. DKIM/DMARC and Sending DNS
records remain externally owned. Destination verification remains a human step.

## Controlled activation and operations

Once this code is on protected main and the execution branch, a reviewed GitHub
plan may set `bootstrap_code_installed=true`, `activation_validated=true` and
individual `enablement` fields. First enable certificates and dispatch
**Deployment preview certificate** on `develop/0.1.0`; inspect the first valid
Secret Manager version. Its daily schedule renews within 30 days, preserves the
ACME account privately, and a normal lifecycle reconcile refreshes running edges.
Then run nonproduction/mail acceptance. Require the release App's staging check
only after the bootstrap main merge; this check affects every main-target PR.
Production additionally requires nonproduction enablement and that staging gate.

Prepare the backup API with `production_backup_agent_enabled=true`. The composed
production config forwards `gke_backup_enabled=true` to the actual deployment
cluster. Only after observing that cluster/agent should a second bootstrap plan
enable `production_backup_enabled` and `production_monitoring_enabled`.
[Operations inspection](gcp/README.md) checks successful backup freshness and alert
configuration; it explicitly does not claim restore success or email arrival.
Keep application provider credentials and external effects disabled during an
isolated restore drill, and protect the independent native Home ledger.

Human acceptance and authorization remain in the
[release runbook](../../docs/operations/release-deployment.md). No helper derives
approval from elapsed time, local tests or a successful Terraform apply.
