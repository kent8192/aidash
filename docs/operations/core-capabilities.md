# Harness-managed capabilities

Core capabilities are independently enabled on an immutable Agent version. They
are subject-authorized operations of the Harness, not Registry tools. External
Registry integrations retain their `plugin_N` identities and credentials.

## Supported execution boundary

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
The fork probe first creates a child, then lowers its own hard process limit to
8 and verifies that further forks receive `EAGAIN`. It records both that probe
limit and the configured inherited limit. The node adapter separately verifies
the exact kernel cgroup ceiling. Admission does not exhaust the entire Pod's
host task budget, which also contains gVisor's own threads.

The pinned acceptance installation uses Kubernetes 1.34, Cilium 1.20.2 and
gVisor 20260921.0. The image pins Python 3.13 and hash-verified Python dependencies
in `runner/requirements.lock`. The runtime image's final OCI digest is recorded
by the setup command and the admission result. Admission is required separately
on each architecture and deployment; a local arm64 result does not certify an
amd64 deployment.

## Reproduce the runtime gate

Requirements: Docker, Python 3 and the repository's Rust toolchain. The wrapper
downloads checksum-verified kind, kubectl and Helm into its own ignored directory.
It creates a dedicated cluster and explicit kubeconfig, chooses a loopback port,
runs the real PostgreSQL/NATS and isolated-runtime acceptance suite, and removes
its own cluster on exit:

```sh
scripts/test-capability-cluster.sh
```

Logs are under `.ignore/core-runtime/<cluster>/evidence/`. Only that directory is
uploaded by CI. Its sibling `private/` contains the runner credential and
kubeconfig and must not be published. `CI Success` requires the runtime gate in
addition to Rust, browser, federation and orchestration checks.

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

Without `AIDASH_CAPABILITY_PROFILE`, admission defaults to disabled. Every Agent's
new capability flags also default to false, and core reconciliation workers do
not allocate database pools. An explicitly configured profile with
`admission: false` keeps reconciliation running for rollback. Production
operators must provision the trusted controller, persistent private
journal/object storage and equivalent
node isolation before setting the profile path on the API and Worker services.
The runner token is referenced by environment-variable name in the profile;
secret bytes belong only in the trusted services' environment.

1. Deploy the additive database migration while admission is disabled. Preserve
   the existing database, working objects and runner journal through upgrades.
2. Run admission and the runtime gate against the deployed isolation profile.
3. Create a new version of a selected test Agent in its existing configuration
   form. Enable the required capabilities, attach authorized Skill directories
   or references, and approve that exact Registry version through the catalog.
   Saving configuration does not grant execution or file permissions.
4. Test with a tenant subject credential. Operator configuration access does not
   become access to users' private working files or originals.
5. Expand Agent/catalog/policy scope explicitly only after the acceptance and
   compatibility evidence passes.

To roll back, restart API and Worker services with the same profile and storage
paths but `admission: false`. Keep the controller and reconciliation workers
running. They stop active operations and warm interpreters, deny new outbound
work/transfer publication, and retain operation journals, receipts, files and
recovery controls. Observe termination and reconciliation states before stopping
the services. Re-enabling admission requires an explicit new Python session
acknowledgement; historical Python code is never replayed to rebuild memory.
Do not down-migrate or remove storage to disable the feature.

## Initial ceilings

| Resource                               | Default                                                          |
| -------------------------------------- | ---------------------------------------------------------------- |
| CPU / memory / processes               | 2 CPUs / 2 GiB / 128                                             |
| Working files / temporary scratch      | 1 GiB / 256 MiB per area                                         |
| Retained objects                       | 10 GiB per tenant, including snapshots and staging               |
| Ordinary / maximum execution           | 120 / 600 seconds                                                |
| Package installation                   | 300 seconds                                                      |
| Idle interpreter                       | 30 minutes                                                       |
| Inline read / search page              | 16 KiB / 50 matches                                              |
| Command / patch input                  | 64 KiB / 256 KiB                                                 |
| Search response / duration             | 32 KiB / 5 seconds                                               |
| Direct Skill package                   | 64 files / 256,000 bytes (existing import ceiling)               |
| Captured operation / broker output     | 8 MiB, with bounded continuation                                 |
| Run permission grant                   | At most 1 hour and no later than Run termination                 |
| Pending approval / uncommitted staging | 24 hours                                                         |
| Cleanup recovery                       | 7 days                                                           |
| Reference set                          | 8 files, 10 MiB each, 200 PDF pages, 64 KiB extracted text total |
| Shared snapshot                        | 64 files, 100 MiB each, 256 MiB total; 4 MiB transfer chunks     |

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
    "share_files": 32,
    "share_file_bytes": 52428800,
    "share_bytes": 134217728
  }
}
```

The 4-MiB chunk frame and bounded management pagination are protocol framing
limits. Transfer negotiation advertises the operator's lowered file/count/total
budgets. Configuration and Run admission both enforce reference-set limits;
lowering a limit never rewrites a published Agent version or deletes originals.

## Files, references, Skills and packages

A working area belongs to an authorized subject, home/workspace/thread and Agent.
Follow-up Runs share files in admission order. Queue adds a future Run; steer
addresses the expected active Run; stop bypasses ordinary queued work. Python
variables live only in the currently acknowledged interpreter. Saved files have
independent immutable identities and remain after an idle or authority reset.

Search returns scope, file digest and source locations. File continuations bind
the area revision; patches require exact preimages and publish one complete
revision. Binary original references stay private and read-only. Python edits a
separate working copy. PDF encryption, unsupported formats, malformed content,
page/text limits and non-extractable content are explicit extraction states.
The original remains downloadable by its authorized owner when the upload was
accepted. OCR, formula recalculation and universal format preservation are not
provided. Text-only legacy references remain text-only until re-uploaded.

Direct Skills use immutable directory manifests. Discovery reveals bounded
metadata; activation loads the chosen instructions, and other files are read on
demand. Name collisions use distinct identities. Loading instructions does not
execute scripts. Existing Registry Skills remain readable through their exact
old identities and authority.

The outbound broker enforces configured HTTPS origins, public DNS/IP checks,
per-hop scope and bounded responses. Shell/Python have no general network proxy.
An approval binds the original requester, invocation, targets and current policy;
upper-policy denial cannot be approved away. Authenticated services belong in
separately configured integrations. Never copy their tokens into Python, Shell,
Skill scripts or package requests.

Additional Python dependencies use explicitly authorized, hash-pinned wheel
objects fetched by the broker from permitted package origins. Installation is
offline in the sandbox, with no dependency auto-download or source build. It
changes only the Agent overlay, records the resolved manifest and resets Python
memory explicitly. Packages are not automatically reinstalled after environment
loss.
If retained storage fills after an operation executes, its poll result exposes
`STORAGE_QUOTA`. The old file revision stays intact and the runner's result is
retained. Restore capacity to reconcile that same operation ID; submitting the
operation again does not reinstall packages or repeat code.

## Sharing and cleanup

Sharing selects exact file IDs/digests and one recipient node/Agent/version/thread.
Both disclosure and receipt authority must permit it. Private-input restrictions
follow derived working files, including after copy or rename. Local and remote
recipients own independent immutable inputs and can materialize their own working
copies. Source edits or cleanup do not recall already received information.

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
intent. Deleted threads leave retained files accessible to current authorized
owners through settings; restoration creates a new thread binding, not old
messages or old authority.

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
between those local threads. Cross-node core flags do not manufacture a local
conversation or authorize private working data.
