# CI coverage uploader

The Rust coverage upload job installs Codecov CLI 11.3.1 from PyPI through the pinned
`codecov/codecov-action`'s official `use_pypi` input. OIDC authentication,
the explicit LCOV input and `fail_ci_if_error: true` remain required.

## Partitioned execution

Eight independent jobs execute the locked Cargo targets: foundation, server unit
and binaries, identity, execution, persistence, collaboration, federation, and
knowledge/marketplace. `scripts/rust-test-partitions.py` inventories the actual
workspace; new integration targets are assigned by their owning application.
Unsupported target kinds fail the inventory instead of silently losing coverage.
`scripts/test-rust.sh --coverage --partition NAME` builds the extension-enabled
PostgreSQL image when required and writes `coverage/rust-NAME.lcov`.

The uploader waits for all eight jobs and checks the exact set of nonempty LCOV
artifacts. It submits all reports together with `flags: rust`; Codecov combines
execution counts for shared source files. An absent or failed partition prevents
the upload. `CI Success` also requires every partition and the upload separately.
Bruno runs all 269 endpoints and 935 scenarios in its own required job; only
sanitized source identity, executable hashes and assertion results are archived.
Both LLVM export and Codecov exclude test directories, `tests.rs`, and sibling
`*_tests.rs` modules so those test bodies do not contribute to application coverage.

The isolated capability gate prepares the same extension-enabled PostgreSQL
fixture before running its library and integration targets. Its Cargo cache is
separate from the eight instrumented coverage partitions.

Before creating the isolated cluster, the gate compiles these exact Cargo targets
and lists their tests. Every acceptance identifier must resolve to a listed test
or its parameterized cases. This catches stale module paths and renamed migration
tests before provisioning. The final reducer still requires successful execution,
the unchanged source fingerprint, isolation admission, and crash recovery;
inventory validation alone cannot pass the gate.

The isolated capability job has a sixty-minute limit. Its cold run at
[`3d05da43`](https://github.com/kent8192/aidash/actions/runs/37337525795/job/111855956903)
spent eleven minutes building, twenty-one minutes passing all 96 capability
tests, and three minutes passing all 24 migration tests before reaching scoped
remote authorization. The former forty-minute limit cancelled that final suite.
The revised budget retains every target, isolation assertion, and artifact check.

The k3s gate saves images to an owned temporary archive, copies it into the owned
node, and imports it through containerd's `k8s.io` namespace. Every expected tag
must then resolve through CRI before acceptance starts. This avoids k3d's Docker
exec stdin transport, which failed with a closed Docker socket in the
remote-memory job. [k3d's image import implementation](https://github.com/k3d-io/k3d/blob/v5.9.0/pkg/client/tools.go)
uses that transport for direct imports; the same error has been reported in
[k3d issue #1020](https://github.com/k3d-io/k3d/issues/1020). The new path retains
all import failures and acceptance assertions and cleans its local archive on
exit; the existing cluster guard owns node cleanup. No new upstream defect is
inferred from the closed socket alone.

Transaction fault cases wait for the prior server generation to terminate after
a rollout and forward HTTP to the single current ready Pod. A successful rollout
can leave old Pods draining; their recovery loops must stop before submitting a
new transaction with a different fault selector. This preserves the required
before/after durable cuts instead of allowing an older controller to complete
the new transaction outside the selected cut.

The required Clippy matrix checks each Cargo workspace once: the backend with
all features, plus the existing desktop and infrastructure observer workspaces.
Each job has its own Cargo cache and denies warnings. Trunk retains formatting
and the other linters; it excludes Clippy in hosted CI because its nearest-package
grouping launched repeated `--workspace` commands against the same build directory.
[The run at `3d05da43`](https://github.com/kent8192/aidash/actions/runs/37337525795/job/111855956995)
reported no lint findings, but two commands exceeded their ten-minute limits
while compiling and waiting for Cargo locks. The explicit matrix avoids those
duplicate invocations and remains a prerequisite of `CI Success`.

## Download outage

On 2026-10-05, [CI run 37242692329](https://github.com/kent8192/aidash/actions/runs/37242692329)
at `43109d948672556110f63040989e12aec0eafee0` completed every Rust test and
generated LCOV, then failed while downloading the CLI from `cli.codecov.io`:

```text
curl: (35) OpenSSL/3.0.13: error:0A000410:SSL routines::sslv3 alert handshake failure
gpg: can't open 'codecov.SHA256SUM.sig': No such file or directory
```

The same endpoint failed TLS negotiation from macOS, while PyPI remained
reachable. [Codecov issue #1975](https://github.com/codecov/codecov-action/issues/1975)
reports the same errors. The upstream report was already open when inspected
on 2026-10-05; no additional upstream report was submitted.

The [official PyPI input](https://github.com/codecov/codecov-action#arguments)
avoids this endpoint. It uses pip's package download checks instead of the
standalone binary's GPG signature verification. The CLI version is pinned
instead of selecting the newest package on each run.

Remove `use_pypi: true` after the versioned CLI binary, checksum and signature
downloads succeed and the normal Action path uploads coverage in CI. Keep
`version: v11.3.1`, `use_oidc: true` and `fail_ci_if_error: true` in that normal
configuration. Closing the upstream issue alone does not establish recovery.

## Validation

The pinned Action's wrapper successfully installed CLI 11.3.1 in a temporary
virtual environment with `CC_USE_PYPI=true`, `CC_VERSION=v11.3.1` and
`CC_DOWNLOAD_ONLY=true`. The installed CLI reported version 11.3.1 and accepted
`upload-coverage --help`. Workflow inputs were checked against that Action's
`action.yml`; formatting and local documentation links passed validation.
An authenticated upload still requires a new GitHub Actions run.

## Independent cluster failure

The same run's `Cluster recovery (k3s, remote-memory)` job failed before its
acceptance tests, while Cargo fetched `web-sys` from crates.io inside the Docker
build:

```text
[55] Failed sending data to the peer (OpenSSL SSL_read: SSL_ERROR_SYSCALL, errno 0)
```

Other cluster jobs built the same backend successfully, and a fresh request to
the `web-sys` index succeeded. Retry this job once the workflow run finishes;
its build failure does not justify changing application behavior or weakening
the acceptance checks.

## Browser dependency mirror

[The Kubernetes remote-memory job at `3d05da43`](https://github.com/kent8192/aidash/actions/runs/37337525795/job/111855957789)
stopped before cluster assertions when Playwright's Ubuntu package installation
reached its ten-minute deadline with exit 124. Its package URLs still used the
Azure HTTP mirror despite the source-list rewrite. The
[runner image configuration](https://github.com/actions/runner-images/blob/main/images/ubuntu/scripts/build/configure-apt-sources.sh)
resolves these URLs through `/etc/apt/apt-mirrors.txt`. The shared
`scripts/configure-ci-apt.sh` helper rewrites the mirror list and direct entries
for browser and desktop lint setup to the Ubuntu HTTPS archive, retaining
priorities, timeout settings, and the installation deadline. No third-party issue
was submitted for this observed transport failure.

The ideal path is the ordinary `playwright install --with-deps chromium` against
working runner sources. Remove the URL rewrite after unmodified hosted sources
complete the same bounded installation reliably across the browser and cluster
jobs; keep the deadline and required acceptance assertions.
