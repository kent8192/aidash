# Portable decision gate foundation

The portable `DecisionGate` provides explicit, version-pinned Compaction decisions
for #108. It is not composed into native agent execution yet. Native authority,
owner accounting, atomic persistence and administration adapters remain required.

Decider declarations require an exact Jev model version and an HTTPS endpoint.
HTTP is accepted only when the URL host is a literal loopback IPv4 or IPv6
address. DNS names, including `localhost`, cannot authorize plaintext disclosure.
The Jev adapter validates the declaration before resolving credentials or
preparing authenticated requests.

`DeciderConfig::digest()` commits to the canonical JSON object containing `model`
and `parameters_digest`. The latter hashes the complete serialized configuration
with `model` replaced by JSON `null`. Historical `Evidence` retains that hash as
`configuration_parameters_digest`, allowing replay to reconstruct and check the
configuration pin using its recorded model. Endpoint and credential references
are not copied into evidence. No current definition, policy or provider call is
needed to verify the commitment.

Replay checks dropped counts against the recorded branches and requires the
truncated count to be no greater than the number of `TruncateResult` branches.
Those branches can leave short results unchanged, so their count is an upper
bound rather than an exact truncation count. The complete candidate must fit
before Enforce can apply it; Shadow records the proposal without applying it.

Rust consumers must include the required `Evidence::configuration_parameters_digest`
field and obtain configuration pins through `DeciderConfig::digest()`. Earlier
development revisions that hashed the configuration directly must reconstruct
their pins from the approved definition; replay does not substitute a model or
default a missing commitment. This foundation provides no legacy compactor
projection or native cutover.
