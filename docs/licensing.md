# Licensing

Aidash is licensed by component. The root [LICENSE](../LICENSE) is a scope
notice; it does not choose one license for the whole repository.

## Community and Enterprise

One rule classifies a component. **A component is Enterprise if it exists only
to run Cloud and Dedicated, the Offerings that Aidash operates. Everything else
is Community.**

Community is licensed under Apache-2.0. It covers the public contracts and the
code needed for a Self-hosted Deployment: `crates/`, `server/`, `web/`,
`desktop/`, `runner/`, `deploy/`, `docs/`, `scripts/` and `.github/workflows`.
`aidash-capability` is a public contract, so it stays Community even though the
Enterprise build uses it.

Enterprise is licensed under BUSL-1.1 with the parameters in
[enterprise/LICENSE](../enterprise/LICENSE). It covers `enterprise/`: the Cloud
backend, the Cloud sign-in UI, the Credential Broker, Cloud infrastructure and
delivery code, and the Enterprise server, dashboard and crates. The Additional
Use Grant is `None`, so only Aidash runs Enterprise in production.

Enterprise source is public so that anyone can read it and report bugs. Only
Aidash runs it in production. Community is complete enough for a Self-hosted
Deployment.

Third-party material keeps its own terms and notices. See
[THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md). A license follows the
material, not its filename or location: moving a file does not change its
license.

## Questions and answers

| Scenario                                                                      | Answer                                                                                                   |
| ----------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------- |
| A company self-hosts Community in production                                  | Permitted under Apache-2.0                                                                               |
| A systems integrator charges for Community installation or support            | Permitted                                                                                                |
| A provider hosts Community as a competing service                             | Not prohibited for competing. Trademark and other terms apply separately                                 |
| Someone evaluates or modifies Enterprise without production use               | BSL's own rights apply, with no invented time limit                                                      |
| Anyone other than Aidash uses Enterprise in production before the Change Date | Not permitted. No commercial license is offered                                                          |
| A Dedicated customer                                                          | Receives a service that Aidash operates on infrastructure Aidash controls. No software license is needed |
| Someone redistributes Enterprise, original or modified                        | BSL notice and scope obligations apply                                                                   |
| Someone sells a tool that integrates through public interfaces                | No Enterprise restriction from integration alone                                                         |
| An Enterprise version reaches its Change Date                                 | That version becomes available under Apache-2.0. Newer versions have later dates                         |

## Earlier notices

Earlier revisions keep the terms they carried. The cutover withdraws nothing
that an earlier revision granted.

- Before the interim notice, the root `LICENSE` held the MIT notice of
  `tamaratran/fast-jev-compaction`. That notice covered only the Jev-derived
  portions, not the rest of the repository. It is kept verbatim in
  [third_party/fast-jev-compaction/LICENSE](../third_party/fast-jev-compaction/LICENSE),
  and the covered files are listed in
  [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md).
- `aidash-broker` and `aidash-capability` declared Apache-2.0 from 2026-10-09.
  `aidash-capability` stays Apache-2.0. Revisions of `aidash-broker` published
  before the cutover keep Apache-2.0; from 0.1.0 it is part of the Licensed
  Work under `enterprise/LICENSE`.
- The interim notice from #250 reserved all rights in original Aidash material,
  except `aidash-capability` and third-party material. Revisions published
  under that notice keep those terms.

## Change Dates

Each Enterprise version has one Change Date. It is four years after the date
that version was first distributed publicly under BUSL-1.1. Enterprise source
that was public earlier under the interim notice carried no license, so it
does not start the clock. The version is under BUSL-1.1 until that date, and
from then it is available under Apache-2.0.

[enterprise/VERSIONS.toml](../enterprise/VERSIONS.toml) records each version's
Change Date and Change License. It also records the first public commit and its
date. For 0.1.0 the cutover PR fixes the Change Date at 2030-10-26, and a
follow-up PR records the commit and its date after the cutover merges. CI derives the
first public commit from git as the first commit that added
`enterprise/LICENSE`, and rejects a record that moves a date later.

Newer versions have later Change Dates, so an older version's conversion does
not put newer code under Apache-2.0.

## Known limitations

- No counsel reviewed these terms. The Licensor approved the final content of
  the cutover (#255) before its first push, and that push made the terms
  public. Nothing in this repository is legal advice.
- Copyright in AI-generated code is uncertain. Much of the code was written by
  AI agents under the author's account, and the Licensor's authority rests on
  being the sole author account.
- The pre-cutover notice was ambiguous. It reserved all rights, but it named
  only one exception and described a layout that was still changing. Earlier
  revisions keep whatever terms they carried.
- The Licensor is pseudonymous. `kent8192` is identified only through the GitHub
  account; no legal name or personal e-mail address is published.

## Contributing and commercial use

[CONTRIBUTING.md](../CONTRIBUTING.md) states the contribution terms for each
component. [COMMERCIAL-LICENSING.md](../COMMERCIAL-LICENSING.md) explains why no
commercial licenses are offered and how Cloud and Dedicated work.
