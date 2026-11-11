# Node capability implementations

Core capabilities are Node-provided operations declared through immutable
Registry descriptors. The Node owns their provider contracts and implementation;
the declarations are built-in definitions in every Tenant's Registry, and the
Sandboxed ones need the Tenant's Catalog Approval before use (ADR 0034).
See the [capability glossary](registry-capability-glossary.md) and
[Registry capability operations](registry-capabilities.md).

Native Agent registration and execution use immutable, qualified Bindings and
Provider descriptors. Required/default Builtins and approved Sandboxed Bundles determine
the exposed tool set. The runtime gate below verifies these implementations. Registry
declarations and catalog approvals do not grant authority over target resources.

## Supported execution boundary (runner/1)

Shell, Python and document parsing require the `aidash-runner/1` controller,
Kubernetes, the `aidash-gvisor` RuntimeClass and the trusted node lifecycle adapter
in `runner/node_guard.py`. The controller runs on a trusted service host; submitted
Agent code runs only as UID 10000 in gVisor. It has no service-account token,
control socket, host mount, node credentials or unrestricted network access.

The controller verifies the image digest, namespace restrictions, effective
network policy, read-only root, capped `/work` and `/tmp`, process limits and
physical interpreter freeze/termination before admitting work. The node adapter
also verifies cgroup CPU/memory/process ceilings and disables swap for the
sandbox cgroup. A Kubernetes pod-deletion response alone is not termination
proof. Missing support makes execution unavailable; there is no host fallback.
A command or Python cell still running at its time limit is stopped, whatever
exit code it then gives. The limit counts from when it starts, so preparing its
sandbox does not use it up. Its result says `COMMAND_TIMED_OUT`, with the limit
and the revision its file changes were saved as. If the save is refused, the
result keeps `WORKING_EXPORT_REFUSED` and sets `deadline_expired`.

When Kubernetes is unreachable, the Runner keeps tracking an accepted command
and retries output, status and cancellation requests. The saved Area remains
readable and busy; a new command receives `AREA_BUSY`. Polling reports the age
of the last output preview. Egress is refused after the command's time limit.
After a further 30-second grace period, the Runner requests a stop and polling
reports that the stop is unconfirmed. The Run may continue with the saved files.

If the stop is still unconfirmed 30 minutes later, the Runner requests a normal
Pod deletion and keeps the Area busy until deletion completes. A completed
normal deletion proves the Pod stopped; an API response accepting deletion
alone does not. If stopped output cannot be read, the Runner retries for 30
minutes, then reopens the Area with its last saved files and reports that the
command's changes were not saved. It continues trying to retain the output
without publishing it for 24 hours by default; the Runner profile's
`uncertain_retention_seconds` overrides this interval. The Helm setting is
`execution.runner.uncertainRetentionSeconds`.

If a Pod will not disappear, a Deployment Administrator should inspect its
deletion timestamp, node health, node guard logs and allocation receipt and
finalizer. Restore the API server, node and guard so the Runner can confirm the
stop and release its finalizer. Do not force-delete a Pod whose node or writer
may still be running; keep the Area fenced until termination can be confirmed.
The fork probe first creates a child, then lowers its own hard process limit to
8 and verifies that further forks receive `EAGAIN`. It records both that probe
limit and the configured inherited limit. The node adapter separately verifies
the exact kernel cgroup ceiling. Admission does not exhaust the entire Pod's
host task budget, which also contains gVisor's own threads.

The pinned acceptance installation uses Kubernetes 1.34, Cilium 1.20.2 and
the gVisor release pinned in `runner/gvisor.json` (20260921.0). The image pins Python 3.13 and hash-verified Python dependencies
in `runner/requirements.lock`. The runtime image's final OCI digest is recorded
by the setup command and the admission result. Admission is required separately
on each architecture and deployment; a local arm64 result does not certify an
amd64 deployment.

## Journal Query

After a Run has offloaded a result, it can retrieve its own Context Journal.
Deployments with a Runner expose `journal_query`; Compose and Desktop without a
Runner expose `journal_search` and `journal_read` instead. Search returns short
matches with Journal sequence numbers, and read returns a bounded JSONL page
for an inclusive sequence range. Results are data, not instructions.

`journal_query` runs Python in a Run-owned kernel separate from the Working
Area kernel, without network access or a writable Working Area. `journal()`
returns the parsed JSONL lines. The export is refreshed before each call, so
`journal()` reads the current Journal even though Python variables persist
between cells. An optional `journal_range` limits the export to inclusive
`from_seq` and `through_seq` bounds. Output is size-limited and treated as data.

The export resolves Research references and substitutes revoked or expired
results. A change in withdrawal state, authority, range, or Runner identity
resets the kernel so its variables cannot retain withdrawn material. The call
reports `SESSION_RESET`; a subsequent call starts with a fresh kernel. The
kernel also ends when the Run ends or its idle session expires. Each completed
Journal read records its range for the offload loop guard.

## Execution Pods (runner/2)

The v2 runtime is opt-in and exposes a separate `aidash-runner/2` control
protocol. Existing shell and Python tools continue to use v1 until they are
retargeted. The server must authorize access to a Scratch Space and pass its
opaque ID, pinned image, CPU, memory, capacity, Docker setting and Tenant ID
when Docker is enabled. The Runner creates one gVisor Pod and one PVC for that
ID. A quota-enforcing storage class with a `Delete` reclaim policy and a
controller deletion finalizer is required; a `Retain` volume is refused. Purge
is acknowledged only after the bound PV disappears. Preserve the Runner journal
across restarts; it records operation and volume identities and prevents replay.
A Runner restart resumes watching normal commands and Python sessions in the same
Pod, including their saved output. Saved output counts against the Scratch Space
disk; if the disk is full, the output is marked incomplete. Docker-mode commands
become interrupted with an unknown outcome; they can still be stopped. A Runner update waits for
foreground commands, then replaces Pods and stops their background processes.

A trusted, read-only injected helper copies the image filesystem into the PVC
on first start, then runs commands as root in that persisted root. `/scratch`
contains user files; the copied system tree lives separately on the same PVC.
Installed packages survive Pod restarts. The Runner receipt reports `image_changed`
when the requested image differs from the image used to initialize the tree;
unknown legacy image identity also reports a change. Reset replaces the system
tree from the requested image and preserves `/scratch`. The change log records cheap mtime/size diffs
at active-command transitions and periodically while commands run. An interval
with one command is attributed to its Run; overlapping commands produce an
`ambiguous` record listing the Runs. The index does not capture every possible
metadata-only change, and a sudden Pod loss can lose the latest interval.

The Runner queues foreground commands beyond eight simultaneous commands per
Pod; background processes have a separate pool of 16. The
default deadline is ten minutes and the maximum is 60 minutes. Foreground
commands waiting for a Run may be cancelled; background processes expose their
starting Run and bounded logs. Commands interrupted by a lost Pod or node have
an unknown outcome and are never replayed during the retry window. Terminal
command records are retained for 24 hours and 15 minutes after completion; unsettled
commands and stops awaiting confirmation are retained. Each Space is limited to
4096 command records. At the limit, new commands are refused until records can
expire, while retries of retained commands still return their prior results.
Python interpreters are per Run.
The Pod stops after 30 minutes without a command or Live preview viewer;
background processes alone do not extend its life. Preview and MCP integration
with the application remains for wave 2.

Admission gates user-image startup on a node guard which binds the Pod UID and
sandbox to host cgroup CPU, memory and pids limits. It independently checks
termination before a new writer starts. Normal-mode commands lose SYS_ADMIN,
NET_ADMIN, NET_RAW and SYS_CHROOT from all capability sets including the bounding set and
run with `no_new_privs`; admission runs a user process to verify this and the
guest process limit. The default v2 guest ceiling scales with Pod memory at one task
per 16 MiB, bounded between 32 and 128 tasks. An operator can override it with
`v2_processes` in the Execution Pod configuration. There is no service-account token. Network admission checks
the effective Kubernetes NetworkPolicies and probes a reachable peer Pod, node,
metadata endpoint and external destination, allowing only the Egress gateway.
It also verifies that a listener in the Execution Pod is reachable locally but
not from the admission canary.
It fails closed when its positive controls cannot be reached. On a cluster with
no metadata server, explicitly set `execution.pods.metadataServerAbsent: true`
(or `--metadata-server-absent` in the setup script) to omit only that positive
control; the metadata isolation probe and other controls remain required. Network probes
currently require IPv4. The CNI must enforce policy against raw and spoofed
packets; Docker-mode admission also runs raw packet negative controls.

In Docker mode, guest root can leave the chroot and modify PVC bookkeeping;
the change log and reset are best-effort. Command results use the controller
exec stream rather than trusting the PVC result files. A nonce-framed result
distinguishes command exit codes from transport failures.

Docker mode requires a separate gVisor RuntimeClass with `net-raw` enabled and
is restricted to nodes labeled and tainted for that Tenant. User processes retain
SYS_ADMIN, NET_ADMIN and NET_RAW inside gVisor and `/var/lib/docker` is tmpfs.
A guest root with SYS_ADMIN can change or bypass the guest PID limit, so that
limit is best-effort in Docker mode. The enforced boundary is the Sentry's host
cgroup CPU, memory and pids limits, verified by the node guard and inaccessible
to guest root. A fork bomb can exhaust resources on the Tenant-dedicated nodes;
it does not acquire resources reserved for another Tenant's nodes.

To enable the v2 controller in Helm, configure `execution.pods.enabled`, a
trusted digest-pinned `helperImage`, `storageClassName`, and the private IP
`proxy.endpoint`, `proxy.namespace` and `proxy.podSelector`. Set `allowDocker`
only after provisioning the separate Docker RuntimeClass and Tenant-dedicated
nodes. The controller performs a disposable admission test before serving v2
requests and reports a typed failure if prerequisites or cleanup fail. For a
local kind installation, `setup-capability-runtime.py --execution-pods` accepts
`--helper-image`, `--storage-class`, `--proxy-endpoint`, `--proxy-namespace`
and one or more `--proxy-selector key=value` arguments; `--allow-docker` is
optional. Provision the enforcing storage class and Egress gateway separately.

For a disposable single-node kind rig, TopoLVM can provide the required fixed-size
filesystem and CSI deletion finalizer. Create a dedicated loop-backed LVM volume
group on the kind node, install TopoLVM with a device class for that group, and
use an ext4 StorageClass with `reclaimPolicy: Delete` and
`volumeBindingMode: WaitForFirstConsumer`. The #714 rig used TopoLVM chart
17.3.1, a 20 GiB sparse loop file and `spare-gb: 0`; its 1 GiB probe passed
node-guard capacity validation and the Runner's deletion-finalizer check, then
the PV disappeared after PVC deletion. In this kind node, udev is not running;
LVM needed `activation/udev_sync = 0` and `activation/udev_rules = 0` to create
device nodes itself. These are node-wide settings, so use this procedure only
on a dedicated disposable node. Record the loop file, device and volume group;
never reuse or remove another workload's devices. This kind result is not a
substitute for the required real-cloud-disk proof before production enablement.

Known limits of this runtime, which no tool uses yet. Keep it disabled in
production until wave 2 of #697 has verified it on a real cloud disk:

- Docker mode has no boundary inside the Pod. A user process keeps `SYS_ADMIN`,
  so it can attack the trusted supervisor, the admission probe and the result
  framing nonce. Only the Tenant-dedicated node and host limits contain it. Do
  not trust Docker-mode admission evidence or result frames against a malicious
  user.
- The image-root init container still runs the helper loader against the Tenant
  image, whose `/etc/ld.so.preload` is honoured. Such code runs without mount
  capabilities and sees only its own `image-root` subPath, and the controller
  refuses a symlinked root before `chroot`.
- Admission, the default guest process ceiling and full-disk recovery passed
  on kind with gVisor and a quota-enforcing TopoLVM PVC. A real cloud disk
  remains a production enablement prerequisite.
- Images that need more than the fixed startup deadline to pull and copy fail.
  There is no progress-based extension (F12).
- Change tracking scans the system tree, with the associated cost and fragility
  (F20). Changes made while no command is tracked have no change-log entry (F31).
- Loader-related image environment variables blanked during initialization cannot
  be restored to guest commands (F24).
- `/v1/activity` does not report v2 work (F27).
- Enabling v2 labels a created sandbox namespace privileged, removing the API
  server's restricted admission check for v1 Pods. Their specs remain hard-coded;
  the label is rendered only with `createNamespaces: true` (F33).

Phase 4 of #697 must resolve the Docker-mode trust boundary and init loader
issues (F7, F8, F10), including the Docker user probe failure, before Docker
mode is used. Full-disk and admission evidence on a real cloud disk is required
before production enables Execution Pods.

## Reproduce the runtime gate

Requirements: Docker, Python 3 and the repository's Rust toolchain. The wrapper
downloads checksum-verified kind, kubectl and Helm into its own ignored directory.
It creates a dedicated cluster and explicit kubeconfig, chooses a loopback port,
runs the real PostgreSQL/NATS and isolated-runtime acceptance suite, and removes
its own cluster on exit:

```sh
scripts/test-capability-cluster.sh
```

Logs are under `private/core-runtime/<cluster>/evidence/`. Only that directory is
suitable for sharing. Its sibling `private/` contains the runner credential and
kubeconfig and must not be published. This runtime gate runs manually outside
CI and is not a prerequisite of `CI Success`.

For an interactive acceptance installation with existing Docker/kind/kubectl/Helm:

```sh
python3 scripts/setup-capability-runtime.py \
  --name aidash-core-demo --directory /tmp/aidash-core-demo --port 18969
python3 scripts/start-capability-runner.py /tmp/aidash-core-demo
```

In another terminal:

```sh
AIDASH_CAPABILITY_PROFILE=/tmp/aidash-core-demo/profile.json \
AIDASH_RUNNER_TOKEN_FILE=/tmp/aidash-core-demo/token \
  scripts/test-capability-runtime.sh
```

The setup script refuses an existing cluster or state directory. It does not
modify the default kubeconfig. Its enabled profile is an explicit acceptance
installation, not a production rollout policy.

## Rollout and rollback

Without `AIDASH_CAPABILITY_PROFILE`, admission defaults to disabled and core
reconciliation workers do not allocate database pools. Agents receive no implicit
Sandboxed tools; approving and binding a Sandboxed bundle is explicit. An explicitly configured profile with
`admission: false` keeps reconciliation running for rollback. Production
whoever runs the Deployment must provision the trusted controller, persistent private
journal/object storage and equivalent
node isolation before setting the profile path on the API and Worker services.
The runner token is referenced by environment-variable name in the profile;
secret bytes belong only in the trusted services' environment.

1. Apply the content-store migrations while admission is disabled. This
   undeployed cutover discards the previous UUID objects and their retained
   references and resets Working Area manifests. It deletes Thread Attachment
   rows with original or WAV object IDs and `reference`, `transfer_in`,
   `transfer_out`, `collaboration`, and `cleanup` records that can name those
   objects. Inline-only attachments and unrelated records remain. Reversal restores the legacy schema but discards content-store objects and
   rows that name them; restore a pre-cutover backup to recover data.
2. Run admission and the runtime gate against the deployed isolation profile.
3. Approve the exact Sandboxed bundles in the Tenant. Register explicit Skill or
   reference Sources, create a Binding-only version of a selected Agent and
   approve that exact Registry version. Saving configuration does not grant
   execution or file permissions. Follow the [drained fleet rollout](registry-capabilities.md#drained-fleet-rollout-and-rollback).
4. Test with a tenant subject credential. A Deployment Administrator holds no Tenant authority and
   reaches no users' private working files or originals (ADR 0102).
5. Expand Agent/catalog/policy scope explicitly only after the acceptance and
   Binding and isolation evidence passes.

The allocation-receipt journal introduces a breaking format change. The supported
format includes the root `instance`, the `allocation-journal` initialization
anchor, and the independent `allocations/` subjournal with its matching `instance`
marker. Pre-allocation journals are unsupported; there is no automatic migration
or supported in-place upgrade from that format. Under an existing runner instance,
a missing anchor or allocation subjournal cannot be distinguished from evidence
lost after initialization, so startup fails closed.

Preserve the original journal, operation records, working objects and available
provider evidence for investigation and manual reconciliation. Do not delete the
journal, reset the runner instance or create replacement allocation markers to
make startup succeed. Missing historical Tenant attribution or provider start/stop
evidence must not be guessed or converted into fabricated usage receipts.

To roll back, restart API and Worker services with the same profile and storage
paths but `admission: false`. Keep the controller and reconciliation workers
running. They stop active operations and warm interpreters, deny new outbound
work/transfer publication, and retain operation journals, receipts, files and
recovery controls. Observe termination and reconciliation states before stopping
the services. Re-enabling admission requires an explicit new Python session
acknowledgement; historical Python code is never replayed to rebuild memory.
Do not down-migrate or remove storage to disable the feature.

## Working Area storage

Working Area objects use SHA-256 content addresses. A Tenant has its own blob
namespace; its Working Areas, retained File versions, receipts and recovery
snapshots can refer to the same bytes without storing or charging them twice.
Each publication creates an immutable Area revision. The current revision and
revisions protected by an active retention hold remain available; the sweep
removes other superseded revisions. A transfer or sharing receipt retains the
File versions it identifies until the receipt record is removed; a recovery
snapshot keeps its versions until it expires or is released. Reclamation
removes bytes only after all references are gone.

Receipt records have no automatic retention or removal policy. Their referenced
bytes remain charged to the Tenant until the records are removed, including
after the associated Working Area is deleted.

The capability profile's `storage` path is a private, persistent local staging
and ownership-intent directory. With the default `blob_storage` setting it is
also the object-store root: Tenant-specific directories contain
`sha256/ab/cd/<digest>`. API and Worker processes sharing a store must also
share the staging and intent directory so recovery can reconcile interrupted
uploads. Keep this directory writable only by the trusted services.

To select a cloud bucket, add one of these objects to the profile:

```json
{ "blob_storage": { "type": "s3", "bucket": "aidash-blobs" } }
```

```json
{ "blob_storage": { "type": "gcs", "bucket": "aidash-blobs" } }
```

The S3 adapter uses the standard AWS environment configuration for region,
credentials and an optional compatible endpoint. It requires atomic
copy-if-absent support through conditional multipart completion. GCS uses
application-default credentials. Give the trusted API and Worker identities
permission to read, write, copy and delete objects in the selected bucket.
Configure a bucket lifecycle rule to remove abandoned multipart uploads and
stale `temporary/` objects after an appropriate recovery interval. Credential
material belongs in the service environment, not the profile.

Retained usage counts each Tenant's distinct blob once, including content kept
by revisions or recovery. In-flight transfer and upload reservations are
accounted separately. Identical bytes in different Tenants remain physically
separate. Tenant purge removes committed and orphaned blob keys, temporary
keys, and local upload intents.

## Initial ceilings

| Resource                               | Default                                                        |
| -------------------------------------- | -------------------------------------------------------------- |
| CPU / memory / processes               | 2 CPUs / 2 GiB / 128                                           |
| Working files / temporary scratch      | 1 GiB / 256 MiB per area                                       |
| Retained objects                       | 10 GiB per Tenant, counted once per distinct blob              |
| Ordinary / maximum execution           | 120 / 600 seconds                                              |
| Package installation                   | 300 seconds                                                    |
| Idle interpreter                       | 30 minutes                                                     |
| Inline read / search page              | 16 KiB / 50 matches                                            |
| Command / patch input                  | 64 KiB / 256 KiB                                               |
| Search response / duration             | 32 KiB / 5 seconds                                             |
| Direct Skill package                   | 64 files / 256,000 bytes (existing import ceiling)             |
| Captured operation / broker output     | 8 MiB, with bounded continuation                               |
| Run permission grant                   | At most 1 hour and no later than Run termination               |
| Pending approval / uncommitted staging | 24 hours                                                       |
| Cleanup recovery                       | 7 days                                                         |
| Reference set                          | 256 files, 10 MiB each, 200 PDF pages, 32 MiB of text in total |
| Extracted text of one PDF or Excel     | 1 MiB                                                          |
| Shared snapshot                        | 64 files, 100 MiB each, 256 MiB total; 4 MiB transfer chunks   |

Working storage cannot exceed the 1-GiB runner export ceiling.
The profile can lower applicable ceilings. Agent input cannot raise them. Disk
and process enforcement is probed rather than inferred from manifest settings.
Content budgets live in the same profile's `limits` object. Omitted keys retain
their defaults; unknown keys, zero values and values above the supported wire
ceilings are rejected. For example, this fragment lowers content budgets while
keeping the deployment's existing runner, storage and resource settings:

```json
{
  "limits": {
    "command_bytes": 32768,
    "patch_bytes": 131072,
    "search_matches": 25,
    "search_bytes": 16384,
    "search_seconds": 3,
    "read_bytes": 8192,
    "skill_files": 32,
    "skill_bytes": 128000,
    "reference_files": 4,
    "reference_bytes": 5242880,
    "reference_pages": 100,
    "reference_text_bytes": 32768,
    "reference_file_text_bytes": 16384,
    "share_files": 32,
    "share_file_bytes": 52428800,
    "share_bytes": 134217728
  }
}
```

Reference uploads require sequential 4-MiB chunks, except for the final
remainder; retries at the same offset remain idempotent. This bounds the number
of staging objects. The 4-MiB chunk frame and bounded management pagination are
protocol framing limits. Transfer negotiation advertises the Node's lowered file/count/total
budgets. Configuration and Run admission both enforce reference-set limits;
lowering a limit never rewrites a published Agent version or deletes originals.

The working-file byte ceiling includes received files. These read-only mounts
reserve capacity from the writable `/work` volume and from every export,
including Shell recovery and persistent Python operations. Filling the writable
allowance therefore keeps the combined manifest within the same ceiling used
when publishing results. Reference files use none of this capacity, because no
Pod receives them.

## Reference sets

A Reference set is all the Reference files one Agent version can read, across
every Source it binds. It does not enter the model's context. Each request says
only how many files there are. An Agent lists their paths with `file_search` in
`path` mode and an empty query under the `references` scope, and reads them with
`file_read`. Neither tool reaches a Pod or the runner. Shell and Python do not
see Reference files. An API client can copy one into the Working Area with
`POST /api/runs/{id}/files/materialize`. An Agent tool for this arrives with the
copy tools of #697.

A Source gives each file a relative `path`, shown to the Agent under
`references/`. Two entries with the same path in one Agent version are refused
at registration, across Sources too. Registering a Source requires being able to
read every file it binds.

Whoever may run an Agent version can have that Run read the Reference files of
its approved Sources, and the Agent needs no `reference.*` action (ADR 0114).
Direct download stays with the uploader, through both the reference and the
working-area routes. Revoking a file is not limited to its uploader: any subject
the Tenant's policy grants `reference.delete` can revoke it, and one the policy
refuses gets 404, as for a missing file. `GET /api/references/{id}/revocation`
gives such a subject the revision and state to revoke with, never the content.
Revoking one file refuses new Runs of every version that
binds it, and the error names its path. A Run that already holds the set stops
reading a revoked file too: `file_read` and the copy refuse it, `file_search`
skips it, and the set's other files stay readable. `file_share` refuses a
Reference file, so no other Agent can keep a copy past its revocation.

`POST /api/references/batch` adds several documents in one request of up to
16 MiB and reports a result per file. The same person adding the same content
gets the existing Reference, unless it is revoked or expired, and two paths with
the same content in one request become one entry. Its `references` field can be
pasted into a `reference_attachments` Source. The request creates no Source,
because a Source needs approval. A larger file uses the chunked upload.

The server decides how a file is read from its content, not its declared type.
UTF-8 text without NUL is ready at once, with no Pod, and Agents read the
original. A file starting with `%PDF-` or `PK` goes to the sandboxed parser.
Anything else keeps its original and is marked `unsupported`.

A PDF page with fewer than 16 non-whitespace characters is marked in the
extracted text, such as `[Page 3: no text]`, and listed in the reference's
`pages_without_text` and on screen. A PDF where every page lacks text is
`non_extractable`. Adding it is not refused, because a page may hold only a
figure. OCR is not performed (#742).

## Files, references, Skills and packages

A working area belongs to an authorized subject, home/workspace/thread and Agent.
Different subjects using the same thread and Agent have separate areas.
Follow-up Runs by the same subject share files in admission order. Queue adds a future Run; steer
addresses the expected active Run; stop bypasses ordinary queued work. Python
variables live only in the currently acknowledged interpreter. Saved files have
independent immutable identities and remain after an idle or authority reset.

Search returns scope, file digest and source locations. File continuations bind
the area revision; patches require exact preimages and publish one complete
revision. Unchanged execution exports reuse their immutable objects; publication
marks superseded working objects for durable reclamation after commit. Outputs
and independent recovery copies retain their own lifetimes. Binary original references stay private and read-only. Python edits a
separate working copy. PDF encryption, unsupported formats, malformed content,
page/text limits and non-extractable content are explicit extraction states.
Searching a specific non-UTF-8 file returns `REPRESENTATION_UNAVAILABLE`.
Scope searches report such files in a bounded `unavailable` list while searching
the remaining text representations. Individual matches exceeding the configured
page budget return an explicit error instead of a non-advancing cursor.
The original remains downloadable by its authorized owner when the upload was
accepted; committing it ends the upload staging expiry. OCR, formula recalculation and universal format preservation are not
provided.
The desktop browser download flow accepts files up to 128 MiB and verifies their
digest incrementally. Inline image preview is limited to 16 MiB; these browser
limits do not remove an authorized original from storage.
Extraction requires the current deployment resource profile to match its
admission probe. A mismatch reports `runtime_unavailable` and waits for repair
without dispatch. Extracted text is additionally bounded by the configured
operation output budget, including JSON escaping; truncated or malformed terminal
results are recorded and acknowledged without leaking the runner payload.
Disabling admission also cancels pending or live reference extraction; it never
starts a queued parser. The original remains available with `admission_disabled`.
Acknowledging completed, failed or cancelled extraction releases the runner's
staged payloads. Python result acknowledgement also releases copied inline images.

Direct Skills use immutable directory manifests. Discovery reveals bounded
metadata; loading brings the chosen instructions into the model request, and
other files are read on demand. Name collisions use distinct identities. Loading
instructions does not execute scripts. Existing Registry Skills remain readable
through their exact old identities and authority.

Under the legacy Exposure policy, `skill_list`, `skill_load` and `skill_read`
behave as before, and `skill_load` marks the Skill loaded in the Run's Skill
record. Under `deferred@1`, those tools are not bound. Direct and Registry Skills
become Discoverable capabilities with `skill_` Model aliases. The model finds them
with `capability_search` and loads their bodies with `capability_load`. A loaded
body is resident in later requests within `skill_bytes`, and `capability_unload`
removes it. The load state is kept only in the Run's `context.exposure`, never in
the Skill record. `skill_asset_read` reads other packaged files by Skill alias,
digest and path under the same `read_bytes` bound, `next_offset` and `truncated`
contract; binary files return metadata only. Direct Skills are not Discoverable
in remote Runs. See
[deferred capability exposure](registry-capabilities.md#deferred-capability-exposure).

The outbound broker enforces configured HTTPS origins, public DNS/IP checks,
per-hop scope and bounded responses. Shell/Python have no general network proxy
in the currently wired runtime. The Scratch Space proxy integration contract and
its remaining authority wiring are described in [Tenant Secrets and sandbox
egress](../tenant-secrets.md#scratch-space-proxy-integration). The `egress`
profile accepts `proxy_tenant_tunnels` (default 32) and
`proxy_global_tunnels` (default 1024) for each gateway instance. The Tenant
value must be positive and strictly less than the global value. Helm deployments
set these with `execution.egress.proxyTenantTunnels` and
`execution.egress.proxyGlobalTunnels`. Scope authority is connected in a later
integration wave.

An approval binds the original requester, invocation, targets and current policy;
upper-policy denial cannot be approved away. Authenticated services belong in
separately configured integrations. Never copy their tokens into Python, Shell,
Skill scripts or package requests.

Git repositories can be cloned through an approved Egress gateway service; see
[GitHub repository access](../tenant-secrets.md#clone-a-github-repository).

Additional Python dependencies use explicitly authorized, hash-pinned wheel
objects fetched by the broker from permitted package origins. Installation is
offline in the sandbox, with no dependency auto-download or source build. It
changes only the Agent overlay, records the resolved manifest and resets Python
memory explicitly. Packages are not automatically reinstalled after environment
loss. The complete package tree and manifest are staged together, then atomically
exchanged with the old overlay. Process death during publication or old-tree
cleanup leaves either the old or complete new package set available.
If retained storage fills after an operation executes, its poll result exposes
`STORAGE_QUOTA`. The old file revision stays intact and the runner's result is
retained. Restore capacity to reconcile that same operation ID; submitting the
operation again does not reinstall packages or repeat code.

### Working Area export restrictions

After each shell command or Python cell, the runner exports `/work` as the next
Working Area revision. It never follows links and never accepts aliases, so it
refuses to export:

- a symbolic link, such as the `node_modules/.bin` links npm writes;
- a hard-link alias, such as the files pnpm links from its store;
- more than 4096 files in the Working Area manifest, not counting Reference
  files;
- more bytes than the configured Working Area quota leaves writable;
- a directory or file whose name is not a printable relative path, or a special
  file.

A refusal fails only that operation, with the tool error
`WORKING_EXPORT_REFUSED`. Nothing is published, so the Working Area keeps its
previous revision and stays usable, and the Run continues. The error message
names the reason, the first offending relative path and, for a quota, the limit.
For example: `symlinks are not supported in /work:
tools/node_modules/.bin/sha.js; use npm install --no-bin-links, or keep
dependency trees under /tmp`. Its `details` carry the same `reason`, `path` and
`limit` fields. The command's output stays readable. A refused Python cell also
resets the interpreter, because its memory no longer matches the published
files. Remove the offending files, install with `npm install --no-bin-links`, or
keep dependency trees and build output under `/tmp`, which is never exported
and is bounded by the temporary-storage ceiling.

## Sharing and cleanup

Deleting a shared thread requires file choices only for the caller's areas.
Other subjects' private files stay unchanged and remain available through their
own settings; conversation deletion does not grant authority over those files.

Sharing selects exact file IDs/digests and one recipient node/Agent/version/thread.
Both disclosure and receipt authority must permit it. Private-input restrictions
follow derived working files, including after copy or rename. Local and remote
recipients own independent immutable inputs and can materialize their own working
copies. Source edits or cleanup do not recall already received information.
Local receipt requires the exact Agent version admitted in the destination's
current generation; a historical Run cannot authorize a newly restored area.

Remote transfer negotiates `file-transfer/1` using explicit mapped subject
identity. Legacy peer authentication alone is insufficient. Prepare reserves
quota, chunks remain invisible, and commit rechecks authority and all digests
before publishing a durable receipt. An uncertain sender reconciles the same
transfer ID; successful upload without the recipient receipt is not delivery.

File management stays under settings and in the existing thread. Keeping files
is the default. Reversible cleanup verifies a recovery snapshot first; a failed
snapshot leaves active files intact. Irreversible deletion requires a separate
revision-bound confirmation and removes only that area's managed copies and
preimages. It preserves originals, published artifacts and independently owned
received copies. A live writer must be proven stopped before cleanup. Partial
deletion remains `cleanup_failed`; use **Reconcile cleanup** to retry the same
intent. If another operation holds a blob lock, deletion stays `deleting` and
the cleanup worker retries it on a later sweep. Deleted threads leave retained files accessible to current authorized
owners through settings; restoration creates a new thread binding, not old
messages or old authority.
Choosing **Keep** for an existing recovery snapshot converts it to retained
storage without expiry. Restoration rechecks the current working-byte limit
before copying or reattaching files. Authorized file administrators can restore
another owner's area subject to current workspace, source and restoration policy;
the original ownership remains unchanged.

## Requirement evidence

`core-capabilities-evidence.json` maps R01–R18 and AT01–AT29 to executable
assertions. A mapping alone is not a passed result. The cluster gate writes
`source.json`, `runtime-admission.json`, `runtime.log` and `result.json` into its
public evidence directory. Missing tests, failed tests, missing admission or
source changes during the run fail the evidence gate. The source record contains
the Git commit, dirty flag and a digest of tracked and untracked source files;
it never identifies an uncommitted test as proof of the base commit alone.
`runner-recovery.json` records an actual controller SIGKILL, repeated deployment
admission and journal recovery. A destroyed fixture Pod with unavailable node
evidence must remain uncertain, while an observed completed operation returns its
saved result exactly once. Missing recovery proof also fails the gate.
`controller-tests.log` covers failure of either admission probe before any
journaled work is resumed and complete, idempotent result-payload acknowledgement.

The browser job retains its Playwright report and `browser-result.json`.
Browser fixtures validate interactions and accessibility; the runtime journey
uses a scripted provider, real PostgreSQL/NATS, real gVisor Shell/Python and two
separately stored nodes. It transfers Python's output and verifies the recipient
copy after the sender's completed cleanup. Network tests use public HTTPBingo
fixtures (no credentials) to test real HTTPS redirects and revocation while a
response is pending. Infrastructure failures remain failed evidence.

Subject-scoped remote workspace execution uses durable source grants and
receiver admissions, with independently rechecked policies at both nodes.
It has no legacy-offer fallback. The task dialog exposes status, pause/resume,
cancel and idempotent additional instructions. Remote workspace Agents use the
workspace tool set; core working-area capabilities require an explicitly
admitted local thread. Use the scoped file-transfer protocol to exchange files
between those local threads. Cross-node Bindings do not manufacture a local
conversation or authorize private working data.
