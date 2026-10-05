# CI coverage uploader

The Rust coverage job installs Codecov CLI 11.3.1 from PyPI through the pinned
`codecov/codecov-action`'s official `use_pypi` input. OIDC authentication,
the explicit LCOV input and `fail_ci_if_error: true` remain required.

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
