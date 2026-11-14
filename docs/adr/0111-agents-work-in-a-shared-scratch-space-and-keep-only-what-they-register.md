# Agents work in a shared Scratch Space and keep only what they register

Decided on 2026-11-02 for #697. Agents work in a **Scratch Space**: a named, persistent but disposable disk with its own settings, which a person picks when starting a conversation and which everyone allowed to use it shares. Each Scratch Space has one long-running, gVisor-isolated Execution Pod, from the image its settings name, in which every command, interpreter session, stdio MCP server and Live preview of its Agents runs. A Working Area keeps only what an Agent explicitly registers from the Scratch Space, in content-addressed object storage. Strict guarantees (exact content, versions, sharing, recovery, Checks) apply to that registered content alone. The Pod reaches the hosts its Scratch Space allows through an HTTPS proxy that records the host and the bytes. Tenant Secrets still never enter a Pod: the Egress gateway attaches them for the External services available to the Scratch Space.

The previous model recreated the Execution Pod for every command, copied and re-hashed every Working Area file into it and exported all of `/work` afterwards. A checked-out repository made each command take minutes (#694), dependency trees and build output failed the export (#682), every ecosystem needed its own package-index adapter (ADR 0071, #679, #695), and one fixed image served every Agent. The Tenant boundary is what must hold, not the boundary between one Tenant's commands.

## Considered Options

- **Keep the disposable Pod per command and add a dependency volume.** Reuses the recovery and evidence model, but background processes, dev servers and interpreter state still cannot survive between commands, and every command still pays for hydration. Rejected.
- **One Pod per Working Area (person × Agent × thread).** Keeps each person's operation tokens out of other people's processes, but every Agent builds its own copy of the same repository and holds its own image. Rejected: build output and images multiply per Agent.
- **One Pod per Workspace.** A new conversation creates a Workspace, so every conversation would start from an empty disk. Rejected in favour of named Scratch Spaces chosen per conversation.
- **Keep naming each destination as an External service and widen the package-index adapters.** Keeps path-level records, but each ecosystem (cargo sparse indexes, Go proxies, apt, Docker registries) needs its own rewriting. Rejected: host-level records are enough for public hosts; path-level records remain for requests that carry a Tenant Secret.
- **A TLS-intercepting proxy.** Would record paths for every host, but the Pod would have to trust Aidash's CA and pinned clients break. Still rejected, as in ADR 0054.
- **Mount each Run's Working Area into the shared Pod.** Other people's Agents in the same Pod could read it. Rejected: deliverables, Thread Attachments and Reference files are copied into the Scratch Space by tools that check the Run's authority.
- **Run each Run's commands as a separate Linux user** to hide per-Run gateway tokens from other processes. Rejected: shared files then depend on group permissions that many tools break. External services with secrets are made available to a Scratch Space instead, so its users share them knowingly.
- **Grant Docker-level capabilities to every Pod.** Lets Agents run Testcontainers, but in-sandbox `SYS_ADMIN` and raw packets widen the gVisor surface a malicious user can probe, while Execution Pods of different Tenants share nodes. Rejected: the Deployment operator allows them per Offering or Tenant, and such Pods run on nodes dedicated to their Tenant. An approval by the Tenant's own people would not protect other Tenants.

## Consequences

- Agents run as root inside the sandbox and may install system packages. These changes persist on the Scratch Space's disk, are recorded with the Run that made them, and a reset returns the environment to its image. A malicious user is the threat model for admission: root inside must expose nothing outside, including the cloud metadata server, other Pods and spoofed packets.
- Run stays one attempt by one agent version at one Task, but it is no longer tied to any container. Background processes may outlive the Run that started them, and a Pod or node failure reports a command as interrupted with an unknown outcome instead of replaying it.
- Egress limits move from per operation to per Scratch Space per day, without a request-count limit. Whether "allow all hosts" is selectable is decided by the operator per Offering; anomalous traffic stops a Scratch Space's outbound traffic.
- Usage (Pod time, disk, traffic) is recorded and billed per Scratch Space, not per Run.
- The `core.npm` and `core.pypi` built-in indexes with response rewriting, the hash-pinned wheel overlay, per-command export of `/work` and the tools that edit a Working Area directly are removed.
- Checks run in a Check Pod from the unmodified image against registered content, with the Scratch Space read-only (amends ADR 0030). Whether the Completion Gate stays is #698.
- Supersedes ADR 0071 and amends ADR 0054.

## Amendment (2026-11-05, #719)

An Agent's Reference set stays readable through `file_read` and `file_search` without being copied, because a copied file is visible to everyone using the Scratch Space. The file tools cover the Scratch Space and the Reference set. Copying remains for a command that needs a file on disk. "Reference files" replaces "Reference Documents" above, because a Reference Document is an Agent's private knowledge, which is sent with every request and is not copied.

## Amendment (2026-11-14, #745)

ADR 0136: each process an Agent starts in the Execution Pod carries a credential for its Run, a Host connection names that Run, and a Run whose read authority is revoked has its commands, Live previews, sessions and Background processes stopped and its traffic refused.
