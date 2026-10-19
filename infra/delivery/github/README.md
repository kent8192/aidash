# GitHub delivery controls

This Terraform root configures the existing `kent8192/aidash` repository. It never
creates or destroys the repository, installs an App, writes a secret, edits a
classic branch protection rule, or adopts unrelated rulesets. The provider is
pinned to `integrations/github` 6.13.0 and locked by `.terraform.lock.hcl`.
Authentication comes only from the provider process's `GITHUB_TOKEN` environment;
the delivery orchestrator obtains it from the existing authenticated `gh` session
without printing it or putting it in Terraform variables.

## Two phases

**Protection phase** is the default (`configure_variables=false`). Supply the
explicit `production_reviewer_user_ids` and the read-only adoption inventory.
Cloud bootstrap/config/App inputs may remain null. This phase creates/adopts five
protected Environments, exact execution-branch policies and two narrow required
CI rulesets. Only the four workflow enablement variables are managed, all false
by default. Existing GCP/App/mail account variables remain untouched.

| Environment                | Only allowed branch            | Protection                                                                       |
| -------------------------- | ------------------------------ | -------------------------------------------------------------------------------- |
| `nonproduction`            | Configured `automation_branch` | No admin bypass                                                                  |
| `production`               | `main`                         | Explicit numeric user reviewers, no admin bypass, prevent self review by default |
| `release-automation`       | Configured `automation_branch` | App key provisioned separately                                                   |
| `deployment-notifications` | Configured `automation_branch` | Sending token provisioned separately                                             |
| `certificate-renewal`      | Configured `automation_branch` | DNS credentials provisioned separately                                           |

`automation_branch` must equal the existing repository default branch and be an
exact `develop/x.y.z`; this root does not change the default branch. Required
reviewers need repository read access. GitHub requires approval from one of the
selected users. With prevent self review enabled, a manually triggered deployment
needs another eligible reviewer; retain that protection or explicitly choose a
review policy appropriate to the authorized production workflow.

**Account phase** (`configure_variables=true`) requires both controller config
JSON strings, both bootstrap objects and existing installed release App metadata.
It validates repository/branch/App identity, deployment tier and project/bucket
isolation, then publishes the fixed workflow variable inventory. The bootstrap
objects contain only project IDs, private bucket names, service-account IDs and
WIF provider IDs, never credential payloads.

Nonproduction bootstrap optionally supplies `preview_tls_renewal` with
`secret_id`, `service_account`, `workload_identity_provider` and
`acme_state_secret_id`. The renewal identities become
`GCP_TLS_RENEWAL_WORKLOAD_IDENTITY_PROVIDER` and `GCP_TLS_RENEWAL_SERVICE_ACCOUNT`.
`acme_email` supplies `AIDASH_ACME_EMAIL`. Nonproduction controller JSON permits
`acme_state_secret`; production permits the boolean `gke_backup_enabled`.
`cloudflare_account_id`, when supplied, is an Environment variable only in
`deployment-notifications`. Both controller JSON strings permit
`credential_brokers` with only bootstrap identity/key IDs, the environment's
`aidash-<id>-cred-` prefix, the config's `byok_project_id`, `enabled` and signing
versions. A broker `image` is refused, PR previews cannot have one, and only the
production config may carry the `production` broker.

Account configuration still leaves every workflow disabled. Explicit
`enablement={nonproduction,production,notifications,certificates}` defaults all
fields false. Turning on any flag requires both `bootstrap_code_installed=true`
and `activation_validated=true`; production additionally requires nonproduction
and the App-bound staging gate. Mail activation requires the account ID, and
certificate activation requires its dedicated WIF identity and ACME email.
These acknowledgements do not execute live probes: the person running the setup
must supply the corresponding acceptance evidence before controlled activation.

Never pass runtime JSON, private keys, DNS tokens, Cloudflare sending tokens or
the private alias-forwarding destination to this root. There are no secret
resources or secret inputs. Provision secrets outside Terraform via the
orchestrator's GitHub CLI helper into their protected Environments. Bootstrap
Secret Manager versions also remain outside this root.

## Preserve existing policy

The only ruleset this root owns is `Aidash delivery main checks`: `CI Success`
from the GitHub Actions App on exactly `refs/heads/main`. Development branches
keep only the baseline `develop/x.y.z` ruleset; this root adds no required check
there.

It contains required status checks only: no review/PR-policy replacement,
branch deletion/force-push changes, administrator exemptions or App bypasses.
Existing broad rulesets and classic branch protection continue to apply through
[GitHub rule layering](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/about-rulesets).
In particular, baseline main/develop rulesets such as IDs `23727469` and
`23727567` must not be imported into these narrow resources.

`require_staging_check=false` initially permits the reviewed privileged CD code
to be installed on main. After account/bootstrap validation, explicitly enabling
it adds `Staging deployment` with `integration_id=release_app.id` to the main
ruleset. GitHub base-branch rules cannot filter on a PR's release head branch:
this applies to **every PR targeting main**, including bootstrap/hotfix PRs.
The flag requires `bootstrap_code_installed` and complete account configuration;
never enable it before those bootstrap changes can pass. Production additionally
validates human release provenance and the staged/main tree independently.

Before any real plan, read current environments, their deployment branch/tag
policies, repository variables and repository rulesets. Build these adoption
inputs from actual metadata:

| Input                                    | Allowed adoption                                                                      |
| ---------------------------------------- | ------------------------------------------------------------------------------------- |
| `existing_delivery_ruleset_ids`          | Key `main`, the ID whose name and narrow semantics exactly match the owned name above |
| `existing_environments`                  | Existing names from the five fixed Environments                                       |
| `existing_deployment_policy_ids`         | One existing exact branch-only policy ID per owned Environment                        |
| `existing_repository_variables`          | Existing names from this phase's fixed managed inventory                              |
| `existing_notification_account_variable` | Existing `deployment-notifications:CLOUDFLARE_ACCOUNT_ID` in account phase            |

Do not select an existing ruleset just because its target branch matches. Stop
on unexpected rules within an owned name instead of losing those controls.
Stop on wildcard/tag/additional deployment policies in an owned Environment
instead of assuming that adding one exact branch policy removes broader access.
Unrelated environments, variables and protections stay outside this root.
Import blocks adopt only the declared IDs; discovery must verify ownership first.
Resources have `prevent_destroy`, so accidental removal/replacement must fail.
Switching an already configured root back to protection-only is not a teardown
mechanism: state-preserving account configuration must remain enabled.

`managed_inventory` exports fixed environment/ref and ruleset-name mappings,
phase-specific repository-variable names and public App metadata for the
orchestrator. It never exports a secret. Use a protected state location even
though this root excludes payloads, and keep generated plan/adoption inputs
private.

## Local verification

```sh
terraform -chdir=infra/delivery/github init -backend=false -input=false
terraform -chdir=infra/delivery/github validate
terraform -chdir=infra/delivery/github test
terraform -chdir=infra/delivery/github fmt -check -recursive
```

The tests use mock providers and `command=plan`; they make no real GitHub changes.
Import syntax is validated, but a mock provider cannot exercise a real import or
prove live reviewer/plan eligibility. Inspect the real read-only plan and current
API inventory before applying through the authorized deployment orchestrator.

Provider schemas and import formats were verified against the primary
[environment](https://registry.terraform.io/providers/integrations/github/6.13.0/docs/resources/repository_environment),
[deployment policy](https://registry.terraform.io/providers/integrations/github/6.13.0/docs/resources/repository_environment_deployment_policy),
[ruleset](https://registry.terraform.io/providers/integrations/github/6.13.0/docs/resources/repository_ruleset),
[repository variable](https://registry.terraform.io/providers/integrations/github/6.13.0/docs/resources/actions_variable)
and [Environment variable](https://registry.terraform.io/providers/integrations/github/6.13.0/docs/resources/actions_environment_variable)
documentation. Environment eligibility and reviewer behavior follow
[GitHub's environment documentation](https://docs.github.com/en/actions/how-tos/deploy/configure-and-manage-deployments/manage-environments).
