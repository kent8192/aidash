# Contributing

Aidash is licensed by component. Read [docs/licensing.md](docs/licensing.md)
before you contribute.

## Community

Community is Apache-2.0. Contributions to Community are accepted under the
Apache License, Version 2.0, inbound=outbound: you keep your copyright and
license the contribution under the same terms as the file you change.

Sign off every commit with the Developer Certificate of Origin 1.1:

```sh
git commit -s
```

The sign-off certifies that you wrote the contribution or have the right to
submit it under the project's license. A DCO sign-off is not relicensing
authority. It does not let the project license your contribution under other
terms.

Community code lives in `crates/`, `server/`, `web/`, `desktop/`, `runner/`,
`deploy/`, `docs/`, `scripts/` and `.github/`. Do not send code for
`enterprise/`.

## Enterprise

Enterprise is BUSL-1.1. It takes issues and bug reports only. We do not accept
external code for `enterprise/` without a signed Contributor License Agreement
(CLA). The CLA is what lets Aidash run the contribution in production under the
Enterprise terms; a DCO sign-off is not enough.

Open an issue to report a bug or request a change. For licensing questions, see
[COMMERCIAL-LICENSING.md](COMMERCIAL-LICENSING.md).

## Third-party material

Third-party material keeps its own terms. Do not remove or change its notices.
When you bring in material that needs attribution, add an entry to
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) and an annotation to
[REUSE.toml](REUSE.toml).
