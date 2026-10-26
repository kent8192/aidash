# Cloud preview domain

This Terraform root manages records in the `aidashusercontent.app` zone, the Cloud
preview domain (ADR 0068, issue #535). The zone itself already exists because the
domain is registered at Cloudflare Registrar, so it is read as a data source and
never created or deleted here.

Today the root manages one record: the `_psl.aidashusercontent.app` TXT record that
the [Public Suffix List](https://github.com/publicsuffix/list/wiki/Guidelines)
uses to validate the private-section entry. Its value is the URL of the
`publicsuffix/list` pull request, so open that pull request first, then apply this
root with its URL. Keep the record for as long as the entry is listed; the
resource has `prevent_destroy`.

The wildcard record `*.aidashusercontent.app` and its certificate are added later,
together with the Preview gateway they point to. Do not publish a wildcard record
before the gateway exists.

## Token

| Token               | Permissions              | Scope                                 |
| ------------------- | ------------------------ | ------------------------------------- |
| Terraform bootstrap | Zone Read, Zone DNS Edit | Only the `aidashusercontent.app` zone |

Do not reuse the `aidash.run` tokens; select only this zone.

## Apply

Applying changes Cloudflare DNS and needs the maintainer's explicit approval.

```sh
umask 077
cp infra/delivery/preview-domain/secrets.example.tfvars infra/delivery/preview-domain/secrets.tfvars
chmod 600 infra/delivery/preview-domain/secrets.tfvars
terraform -chdir=infra/delivery/preview-domain init -input=false
terraform -chdir=infra/delivery/preview-domain plan -var-file=secrets.tfvars -out=psl.tfplan
terraform -chdir=infra/delivery/preview-domain apply psl.tfplan
dig +short TXT _psl.aidashusercontent.app
```

State and plan files stay private; the bootstrap token is ephemeral and absent from both.

Validate without credentials:

```sh
terraform -chdir=infra/delivery/preview-domain init -backend=false -input=false
terraform -chdir=infra/delivery/preview-domain validate
terraform -chdir=infra/delivery/preview-domain test
```
