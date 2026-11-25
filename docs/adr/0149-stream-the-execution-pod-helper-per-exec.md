# Stream the Execution Pod helper per exec

Decided on 2026-11-25 in the long-running execution grilling (#1138). The
Execution Pod container and its read-only helper mount used the same image as
the Runner. Updating helper code changed that digest and replaced the Pod.

## Decisions

- **Serve helper code from the Runner.** Each exec carries a bounded program
  frame on stdin, followed by the operation's input. A small launcher loads
  the code in memory. A running supervisor keeps the code it started with.
  The stable base supplies Python, its loader and libraries, and the code
  that prepares the Pod and enters the guest root.
- **Version the Runner–helper protocol with SemVer.** A major version changes
  an incompatible contract, a minor version can add fields, and a patch
  version fixes behavior. Readers ignore unknown fields. The Runner accepts
  the current and preceding major versions, with any minor version in either.
- **Defer base updates while time-limited Background processes need the Pod.**
  The deferral lasts at most 24 hours. The Deployment Administrator may apply
  a base update immediately. A base replacement or an incompatible running
  helper protocol records `pod_updated`.

## Considered Options

- **Keep helper code in the image.** Even a small helper fix would interrupt
  running work.
- **Write a new helper into the Pod.** A writeable location introduces
  another boundary to protect and needs coordination with running commands.
- **Run Background work in a separate Pod.** That leaves the helper update
  problem on both Pods, while adding a second runtime per Space.

## Consequences

- The Deployment publishes a digest-pinned Pod base and can update the
  Runner independently.
- The exec input includes code as well as request data. The launcher
  consumes the program frame before the helper reads its input, including
  result proof keys and proxy credentials.
- Compatibility tests exercise prior helper versions. The Runner keeps the
  original supervisor and its results during a compatible rollout.
