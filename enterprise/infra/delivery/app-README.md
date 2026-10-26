# Release GitHub App setup

The helper supplies a least-privilege GitHub App manifest and handles code
exchange/key storage. GitHub requires the owner's interactive **Create App** and
**Install App** consent; Terraform cannot perform these actions. Use the existing
`kent8192` authenticated browser session and select **Only select repositories →
aidash**. Do not install on all repositories or add permissions.

```sh
python3 enterprise/infra/delivery/control/app.py register
```

Open the printed loopback URL while the helper runs. Submit its form, review the
manifest on GitHub and create the private App. The callback exchanges its
short-lived code directly with GitHub, with a random one-hour session and CSRF
state. It saves `release-app.pem` and `metadata.json` under the ignored
`secrets/delivery/operator/github-app/`, with directory 0700 and files 0600.
Provider responses and OAuth codes never enter access logs. Then use the displayed
App installation URL, selecting only `kent8192/aidash`.

```sh
python3 enterprise/infra/delivery/control/app.py finalize
```

Finalize signs an App JWT with the private key, verifies exact owner/permissions,
one selected installation and one accessible Aidash repository, then revokes its
metadata-only verification token. It checks live `release-automation` Environment
protection and writes the key directly to `RELEASE_APP_PRIVATE_KEY` through `gh`
stdin. Public IDs go into `release_app.tfvars.json` for `setup.py compose`.

The manifest grants Actions read, Checks write, Contents write, Metadata read and
Pull requests write. It requests no webhooks, events, user OAuth grant,
administration or secret-management capability. These are the permissions needed
for immutable release branches/PRs and the App-owned staging check.

Rerun finalize after installation completes; it does not register another App.
Registration refuses an existing key. If creation succeeded but callback failed,
inspect GitHub's App settings and recover that App/key rather than registering a
duplicate. Keep the local key private and retain/revoke it according to the
your recovery policy after GitHub handoff.

[Manifest registration](https://docs.github.com/en/apps/sharing-github-apps/registering-a-github-app-from-a-manifest)
and [App REST API](https://docs.github.com/en/rest/apps/apps).
