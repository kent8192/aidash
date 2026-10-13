# GCP delivery bootstrap and production operations

The two `.tfvars` examples are explicit inputs for `infra/gcp/bootstrap`.
They preserve separate production/nonproduction projects, state buckets, release
buckets, notification buckets, BYOK projects and identities. Replace every
placeholder before applying. Both examples leave delivery disabled.

`delivery_configuration` combines public flow inputs with provisioned GCP
identities in the Terraform output of the same name. It includes the exact
Cloudflare zone, protected automation branch, main controller branch, Release
App bot, preview TLS secret, ACME account state secret, fixed production origin,
node sizing, registry and the identities/buckets required by deployment and
notification workflows. Secret values, App private keys, email tokens and TLS
private keys are not inputs to this object or its output. Null preserves a
standalone bootstrap that does not emit a publishable delivery config.

Initial bootstrap creates no backup plan or monitoring resources. After the
existing production cluster has been observed, the delivery orchestrator must
enable its `gke_backup_enabled` environment input and observe the installed
Backup for GKE agent. A second reviewed bootstrap plan can then set
`production_backup_enabled` and `production_monitoring_enabled` to true. The
backup resource checks the actual cluster agent before creation. It never
creates another cluster.

The backup plan captures only `aidash-production`, including namespaced config,
Secrets and PVC volume data. Daily backups default to 19:00 UTC (04:00
Asia/Tokyo), 30-day retention and a 7-day delete lock. The plan is protected
against deletion; its retention policy remains adjustable. These settings are
explicit inputs. Google encrypts all backup artifacts at rest by default.
[Backup plans](https://docs.cloud.google.com/kubernetes-engine/docs/add-on/backup-for-gke/how-to/backup-plan),
[agent installation](https://docs.cloud.google.com/kubernetes-engine/docs/add-on/backup-for-gke/how-to/enable-gke-cluster),
[default encryption](https://docs.cloud.google.com/kubernetes-engine/docs/add-on/backup-for-gke/concepts/about-cmek).

Without a ProtectedApplication that coordinates application hooks, PVC backups
are crash-consistent. Configuration and successful snapshot completion do not
prove that PostgreSQL, JetStream, Runner journals or the independent native Home
ledger can be recovered together. Retain and validate the Home ledger
independently of PostgreSQL. A restore test must disable production peers,
provider credentials and external side effects before the restored workloads
start. This bootstrap does not restore into production or create a restore
cluster. [Application consistency](https://docs.cloud.google.com/kubernetes-engine/docs/add-on/backup-for-gke/how-to/protected-application).

Monitoring sends alerts only to `prod.deployment@aidash.run`. The four policies
cover HTTPS `/health` failures from at least two checkers for five minutes,
retained PVC utilization above 80%, five container restarts within five minutes,
and failed Backup for GKE completions. Namespace filters select only production
workloads. The health check requires valid TLS, status 200 and production node
content. Missing or stale backups require an independent freshness check;
absence of failure notifications is not evidence of a recent successful backup.
[Uptime checks](https://docs.cloud.google.com/monitoring/uptime-checks),
[GKE metrics](https://docs.cloud.google.com/monitoring/api/metrics_kubernetes),
[Backup for GKE metrics](https://docs.cloud.google.com/kubernetes-engine/docs/add-on/backup-for-gke/how-to/monitoring-metrics).

For read-only inspection, save `terraform output -json` in a private file and
run:

```sh
python3 infra/delivery/gcp/inspect_operations.py --terraform-output /private/path/production-output.json
```

The command uses `GOOGLE_OAUTH_ACCESS_TOKEN` when supplied, otherwise the
explicit project's active gcloud identity. It performs only API GETs: cluster,
backup plan, paginated backup inventory, uptime check, channel and alert policy
metadata. It rejects paused plans, stale capture times, missing volume data,
changed destinations and alert/TLS drift. The JSON observation explicitly keeps
`restore_verified` and `notification_delivery_proven` false. They require
separate recovery and actual email-arrival evidence. Neither the inspection nor
Terraform configuration changes GitHub enable flags or approves a release PR.
