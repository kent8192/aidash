# Cloudflare mail setup

This Terraform root manages exactly four literal Email Routing aliases:
`stg.deployment@aidash.run`, `prod.deployment@aidash.run`,
`test.deployment@aidash.run`, and `licensing@aidash.run`. The three deployment
aliases forward deployment notifications; `licensing@aidash.run` is the public
licensing contact route. All four forward to one supplied, already verified
private destination. It creates no catch-all and does not send verification or
deployment mail.

The deployment rules are named `Aidash <tier> deployment notifications`. The
licensing rule is `Aidash licensing inquiries`.

The existing zone, configured `aidash.run` Email Sending registration and verified
destination remain data sources. Existing matching aliases are discovered and
imported automatically; ambiguous ownership, Worker-managed rules and unexpected
matchers fail closed. Unrelated aliases are preserved. No Sending registration
is onboarded again, and its DKIM/DMARC/bounce records remain externally managed.

## Tokens to issue later

Use separate credentials for infrastructure setup, outbound notifications and
ACME. Do not reuse the notifier credential for DNS or provide any of these
credentials to PR code. Tokens need explicit account/zone selection; do not select
all accounts or all zones.

Create account-owned tokens from **Manage account → Account API tokens → Create
Token**, choose a name and expiration, then **Continue to summary → Create
Token**. Creation requires token provisioning capabilities or Super Administrator
rights. For a one-time personal bootstrap token, use **My Profile → API Tokens →
Create Token → Create Custom Token**. Both flows are described in Cloudflare's
[account token instructions](https://developers.cloudflare.com/fundamentals/api/get-started/account-owned-tokens/)
and [custom token instructions](https://developers.cloudflare.com/fundamentals/api/get-started/create-token/).

| Token                              | Permissions                                                                                                                                   | Scope                                                                          |
| ---------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------ |
| Terraform bootstrap                | Zone DNS Read, Zone Read, Zone Settings Read; Zone Email Routing Rules Edit; Account Email Routing Addresses Read; Account Email Sending Read | Only the account containing `aidash.run`, and only its `aidash.run` zone       |
| Optional guarded Routing DNS setup | Add Zone Settings Edit and Zone DNS Edit to the bootstrap token                                                                               | Only the `aidash.run` zone                                                     |
| Notification sender                | Account Email Sending Edit                                                                                                                    | Only the account containing the configured Sending domain                      |
| Preview ACME DNS challenge         | Zone DNS Edit and Zone Read                                                                                                                   | Only the `aidash.run` zone                                                     |
| Optional token provisioning helper | Add Account API Tokens Read/Edit to a temporary bootstrap credential                                                                          | Only the explicit account; remove this broad provisioning credential after use |

The API labels the routing rule write permission `Email Routing Rules Write`
and the destination read permission `Email Routing Addresses Read`; the dashboard
uses **Edit** for write access. A verified destination is read only, so address
Edit is unnecessary here. If that destination must be created separately, add
Account Email Routing Addresses Edit for that operation and complete its email
verification before this root is applied. If Email Sending Read is unavailable
in the live permission selector, stop and check the account's available groups;
do not silently substitute a broad account permission.

Email Sending Edit authorizes outbound sending from configured domains on the
selected account. It cannot be restricted to one sender by the token alone; the
trusted notifier enforces `deployment.noreply@aidash.run` and the three recipients.
See the [Email Sending authentication documentation](https://developers.cloudflare.com/email-service/api/send-emails/smtp/).

Copy token values only into the private local delivery input when issued. Do not
paste values into chat, shell command arguments, committed files, PRs or logs.
The notifier token belongs in GitHub's protected `deployment-notifications`
Environment secret `CLOUDFLARE_EMAIL_SENDING_TOKEN`; it is not a Terraform token
resource. The preview ACME token is a separate secret for the trusted TLS edge.

## Private inputs and adoption

Terraform 1.14 or newer is required. The provider is pinned to Cloudflare 5.26.0,
matching the application's reviewed provider version. Its bootstrap token input
is both sensitive and ephemeral: Terraform omits its value from saved plans and
state. If it is null, the provider uses `CLOUDFLARE_API_TOKEN` when supplied.

Prepare a local file without actual credentials entering version control:

```sh
umask 077
cp enterprise/infra/delivery/cloudflare/secrets.example.tfvars enterprise/infra/delivery/cloudflare/secrets.tfvars
chmod 600 enterprise/infra/delivery/cloudflare/secrets.tfvars
terraform -chdir=enterprise/infra/delivery/cloudflare init -input=false
terraform -chdir=enterprise/infra/delivery/cloudflare plan -var-file=secrets.tfvars -out=mail.tfplan
```

Fill `account_id`, `zone_id`, `destination_address_id`, `destination_email`,
`sending_subdomain_id` and the bootstrap credential locally. IDs must be the
actual 32-character identifiers; this root does not infer or repair IDs. The
private destination appears in state, so state and plan files remain private
even though tokens are omitted. Do not enable `TF_LOG` while credentials are
present. Apply only the reviewed saved plan through the delivery orchestrator.

`cloudflare_setup.discover_existing` can resolve the existing Sending registration
and verified destination IDs using read-only APIs, and returns private tfvars in
memory. It refuses unverified destinations and never creates verification mail.

With `configure_routing_dns = false`, Routing must already be ready and DNS is
untouched. With explicit opt-in, the official `cloudflare_email_routing_dns`
resource enables Cloudflare-managed Routing DNS, importing existing enablement
instead of adding it again. It first reads Cloudflare's required DNS proposal and
the exact apex records. Existing external MX, a different or duplicate SPF, an
incomplete inventory, or an incomplete proposal blocks setup. Review any mail
provider migration or SPF merge separately; do not delete records to bypass these
guards. Sending DKIM, DMARC and bounce-domain records are not modified.

## Optional one-time Sending token provisioning

The provider supports `cloudflare_account_token`, but its one-time `value` is a
sensitive state attribute. This root deliberately does not instantiate it. The
optional `cloudflare_setup.provision_sending_token` helper discovers the exact
selectable account Email Sending permission group, scopes a 180-day token to the
explicit account and hands its value directly to `gh secret set` on stdin.

Its default `apply=False` performs reads only. An existing protected GitHub secret
prevents token creation unless rotation is explicitly selected. A successful
creation records only token ID, account ID, expiry and handoff state in the
orchestrator's private journal. If GitHub handoff fails, the new token is revoked;
if revocation fails, the journal identifies cleanup work. An ambiguous create
response can leave a token whose value cannot be recovered; subsequent execution
refuses another token with the same ownership name until it is reconciled.
Rotation preserves old tokens for deliberate retirement after delivery proof.

No live Cloudflare API mutation or email send is needed to validate this change:

```sh
terraform -chdir=enterprise/infra/delivery/cloudflare init -backend=false -input=false
terraform -chdir=enterprise/infra/delivery/cloudflare validate
terraform -chdir=enterprise/infra/delivery/cloudflare test
python3 -m unittest discover -s enterprise/infra/delivery/control/tests -p test_cloudflare_setup.py
```

Provider contracts: [Routing rules and imports](https://registry.terraform.io/providers/cloudflare/cloudflare/latest/docs/resources/email_routing_rule),
[verified destinations](https://registry.terraform.io/providers/cloudflare/cloudflare/latest/docs/data-sources/email_routing_address),
[Sending registration](https://developers.cloudflare.com/api/terraform/resources/email_sending/),
[Routing DNS setup and import](https://raw.githubusercontent.com/cloudflare/terraform-provider-cloudflare/v5.26.0/docs/resources/email_routing_dns.md),
[account token creation](https://developers.cloudflare.com/api/resources/accounts/subresources/tokens/methods/create/)
and [permission discovery](https://developers.cloudflare.com/api/resources/accounts/subresources/tokens/subresources/permission_groups/methods/list/).
