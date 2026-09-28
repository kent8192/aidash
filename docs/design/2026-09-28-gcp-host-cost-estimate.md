# GCP host VM cost estimate

Updated on 2026-09-28 after the user deferred the main production environment.
The initial scope is staging and test environments controlled through GitHub
Actions. The previous continuously running production-host estimate is
superseded by this model.

Q27-Q28 selected shared development staging on normal VMs, test and PR staging
on Spot VMs, and Iowa (`us-central1`). The mixed case below is the current
planning case; other cases remain comparisons.
Q29 selected manual recovery and manual normal-VM fallback, so no automatic
conversion to higher-priced normal uptime is assumed.

## Proposed host layout

Use one VM per running application environment: shared development staging,
one active PR staging environment, and one test environment. Each host runs its
application, PostgreSQL, NATS, Qdrant, Kubernetes, trusted Runner controller, and
isolated Shell/Python Pods. Start the whole environment and pass readiness
probes before making it available; stop it after an idle hour.

This candidate needs no continuously running production, database, or Runner VM.
Infrastructure automation can run in GitHub Actions and short-lived control
services. The cost comparison uses `e2-standard-4` (4 vCPUs, 16 GiB) to leave
room for both services and sandbox workloads. Sizing and simultaneous sandbox
capacity still need validation; the figure is not a measured requirement.

## Price basis

- Selected region: Iowa, `us-central1` (Q28).
- Month: 730 hours for retained disks. Compute uses actual summed VM uptime.
- JPY display: USD multiplied by JPY 160, rounded for planning. The BOJ report
  for 2026-09-25 at 17:00 JST reported USD/JPY 158.09-158.11. Actual Google Cloud
  billing uses its billing-currency SKU prices, not this spot-FX conversion.
- No commitments, trial credits, or enterprise discounts. Amounts exclude tax.
- Rates were checked on 2026-09-28. Current Spot prices can change daily;
  recheck before provisioning.

| VM                                | Normal USD/hour | Spot USD/hour | Normal JPY/hour at 160 | Spot JPY/hour at 160 |
| --------------------------------- | --------------- | ------------- | ---------------------- | -------------------- |
| `e2-standard-2`, 2 vCPUs / 8 GiB  | 0.06701142      | 0.040212      | 10.72                  | 6.43                 |
| `e2-standard-4`, 4 vCPUs / 16 GiB | 0.13402284      | 0.080424      | 21.44                  | 12.87                |

The checked E2 Spot rate is approximately 40% below the normal rate. The current
maximum-discount advertisement is not a promise of a 60% minimum discount.
Older cached pages quoted a different E2 price and monthly price changes;
this model uses the current Spot page's E2 table and daily-change policy.
The current E2 table was read directly from the official page after the web
extractor timed out.

Sources: [Compute Engine prices](https://cloud.google.com/products/compute/pricing/general-purpose),
[Spot VM prices](https://cloud.google.com/spot-vms/pricing),
[E2 specifications](https://docs.cloud.google.com/compute/docs/general-purpose-machines),
and [BOJ exchange-rate report](https://www.boj.or.jp/statistics/market/forex/fxdaily/fxlist/fx260925.pdf).

## Monthly planning example

Assume shared development staging runs for 60 hours, the test environment for
30 hours, and PR staging for 30 hours: 120 billable VM-hours in total. These
hours include startup, readiness checks, active Agent work, and the one-hour
idle tail. An inactive Python interpreter alone does not extend the environment
inactivity deadline. These are example usage, not observations
or an approved quota.

A host contains its execution Pods in this model, so there is no additional
execution-node line to add for the same work. If later measurements require
separate or extra execution nodes, add their hours instead of claiming they
are covered by this estimate.

| Provisioning choice                            | Normal VM-hours | Spot VM-hours | VM compute USD | VM compute JPY |
| ---------------------------------------------- | --------------- | ------------- | -------------- | -------------- |
| All normal                                     | 120             | 0             | 16.08          | 2,573          |
| Shared development normal; test and PR on Spot | 60              | 60            | 12.87          | 2,059          |
| All Spot                                       | 0               | 120           | 9.65           | 1,544          |

## Retained disk and IP subtotal

Assume 30 GiB of balanced persistent disk per environment, including boot and
private application data: 90 GiB retained for the month. Stopping compute
preserves this data. Explicit destruction removes owned disks.

For the comparison, each running VM has an ephemeral external IPv4 that is
released on stop. Stable DNS names can be updated after start; a stopped
environment does not need a separately reserved static IP. The ingress and
OAuth configuration targets are recorded in the [deployment design](2026-09-27-gcp-deployment.md#acquired-domain-dns-and-oauth-origins);
their runtime verification is still required. This assumption does not authorize
public access to database, Kubernetes, or Runner control ports.

| Cost at 120 total VM-hours            | All normal    | Mixed normal/Spot | All Spot      |
| ------------------------------------- | ------------- | ----------------- | ------------- |
| VM compute                            | JPY 2,573     | JPY 2,059         | JPY 1,544     |
| Retained disks, 90 GiB for 730 hours  | JPY 1,440     | JPY 1,440         | JPY 1,440     |
| Ephemeral external IPv4 while running | JPY 96        | JPY 72            | JPY 48        |
| VM + disk + IP subtotal               | **JPY 4,109** | **JPY 3,571**     | **JPY 3,032** |

Balanced disk is USD 0.000136986/GiB-hour. Standard VM external IPv4 is
USD 0.005/address-hour; Spot VM external IPv4 is USD 0.0025/address-hour.
Each extra suspended PR with a 30 GiB disk adds approximately USD 3, or JPY 480
per month, even though only one PR staging environment may run at a time.

Sources: [Disk prices](https://cloud.google.com/compute/disks-image-pricing),
[IP pricing and stopped-instance rules](https://cloud.google.com/vpc/network-pricing),
and [Compute Engine stop behavior](https://docs.cloud.google.com/compute/docs/instances/stop-start-instance).

These are subtotals. Registry storage, data transfer, logs, DNS, secrets,
Terraform state, infrastructure-control services, GitHub Actions charges if
applicable, and tax are additional. No continuously running host, managed load
balancer, Cloud NAT gateway, Cloud SQL instance, or GKE management fee is in this
self-managed VM model. Add the corresponding cost if the final design uses one.

## Usage sensitivity

The mixed case assumes half the VM-hours are normal and half are Spot. Every
scenario retains the same 90 GiB of disk, and all VM types remain 4 vCPU / 16 GiB.

| Total VM-hours/month | All normal: VM only | All Spot: VM only | All normal: VM + disk + IP | Mixed: VM + disk + IP | All Spot: VM + disk + IP |
| -------------------- | ------------------- | ----------------- | -------------------------- | --------------------- | ------------------------ |
| 50                   | JPY 1,072           | JPY 643           | JPY 2,552                  | JPY 2,328             | JPY 2,103                |
| 120                  | JPY 2,573           | JPY 1,544         | JPY 4,109                  | JPY 3,571             | JPY 3,032                |
| 300                  | JPY 6,433           | JPY 3,860         | JPY 8,113                  | JPY 6,767             | JPY 5,420                |

With all environments stopped, this example has zero running-VM compute and
ephemeral-IP cost but approximately JPY 1,440/month of retained disks, plus any
other retained or control-service resources.

```text
Compute USD = 0.13402284 * normal_VM_hours + 0.080424 * Spot_VM_hours
Disk USD = 0.000136986 * 90 * 730
IPv4 USD = 0.005 * normal_VM_hours + 0.0025 * Spot_VM_hours
Planning JPY = (Compute USD + Disk USD + IPv4 USD) * 160
```

The budget target remains JPY 10,000 for all environments together, with LLM and
external API use separate. The 120-hour cases leave more room for the unpriced
items than the previous production-host model, but they are not a guaranteed
billing cap. Retries after Spot interruption increase node-hours.

## Spot suitability

Spot is a discounted, interruptible provisioning model, not CPU-utilization-only
billing. Both normal and Spot VMs accrue compute charges while running, even
when their application is idle.

Compute Engine can reclaim a Spot VM at any time, and Spot capacity can be
unavailable when requested. Keep persistent disks independent from VM deletion,
use a stop termination action for resumable environments, and retain the
existing Python reset-acknowledgement and uncertain-operation recovery contracts.
Do not promise safe automatic replay of interrupted Agent or Shell/Python work.

Q27 selected normal VMs for shared development staging and Spot VMs for tests
and PR staging. Q29 selected manual resumption after Spot unavailability or
preemption and an explicit manual choice for normal-VM fallback. Do not replay
interrupted Shell/Python work through infrastructure automation. Every fallback
hour uses the normal rate, and every retry adds its actual billed uptime.
A manual stop/delete must never be reversed by recovery automation.

Source: [Spot VM behavior and pricing policy](https://docs.cloud.google.com/compute/docs/instances/spot).

## Acquired domain and DNS supplement

The user purchased `aidash.run` on 2026-09-28. The registry identifies Cloudflare
as its registrar. Registration and renewal are paid to that registrar, separately
from GCP; the actual purchase amount and Cloudflare renewal price have not been
verified. The earlier Porkbun offer of USD 4.12 initially and USD 22.14 on renewal
was comparison pricing and is not the user's Cloudflare invoice.

Cloudflare Registrar requires Cloudflare nameservers. The revised proposal keeps
the existing Cloudflare zone and uses DNS on its Free plan, with Terraform
managing the three environment records. This layout adds no paid Cloud DNS zone;
the previous Cloud DNS supplement and JPY 3,898 combined estimate are superseded.
Paid Cloudflare services are not assumed, and the account's selected plan has
not been inspected.

Using the Free-plan DNS proposal, the mixed 120-hour VM + disk + IP subtotal
remains approximately JPY 3,571/month. Add the actual annual domain renewal
divided by 12 for an amortized domain amount, plus the other unpriced services
and tax. This is still not a complete monthly bill or spending cap.

Sources: [aidash.run registry record](https://rdap.identitydigital.services/rdap/domain/aidash.run),
[Cloudflare Registrar requirements](https://developers.cloudflare.com/registrar/get-started/register-domain/),
and [Cloudflare DNS availability](https://developers.cloudflare.com/dns/).
