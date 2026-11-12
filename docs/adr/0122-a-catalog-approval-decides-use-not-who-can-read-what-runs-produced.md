---
status: accepted
---

# A Catalog Approval decides use, not who can read what Runs produced

Decided on 2026-11-07 for #769. A Catalog Approval says that one exact version may be used on one
Node (ADR 0029). Until now the read check of a Run's outputs also went through it: every
definition a Run had read, whether through `agent_discover`, its own Binding or the outputs of
another Run, had to be approved and admitted by the approval's restriction. So upgrading an Agent
by withdrawing its old version paused Runs that had only listed it, hid the results of every Run
that read theirs, and hid the results of the old version's own Runs, finished or not, until it was
approved again. We decided that a read check never looks at an approval or its restriction.
Whether something can be read depends on `registry.read` alone, for every read dependency. What a
withdrawal stops is use: a Run whose Binding names the version stops at its next worker boundary,
cannot be resumed, and its running commands and operations are withdrawn as they are today. The
check that does this is the **Use Recheck**: the Run's own Agent (`agent.execute`) and every
definition of its Binding snapshot (`registry.read`) under the current approval and policy. It
runs at each worker boundary, on resume and wherever a running command or operation of the Run is
rechecked.

## Considered Options

- **Keep the approval in the read check.** A withdrawal then hides far more than the Runs that
  use the version, and a restriction that names certain Agents makes the outputs of every Run
  using the dependency unreadable to every person, because only the Run's own worker may stand in
  for those Agents. Rejected.
- **Treat a listing as not a read.** A reader without `registry.read` on a definition could then
  read outputs drawn from it, and the rows of a listing would have to be told apart from memory
  reads. A Run that delegated to the version would also stay stopped. Rejected.
- **Use `registry.read` only for listings.** It fixes auth-dev and restaurant-dev, whose Runs only
  read a Run of the version. It leaves the architect stopped, which delegated to the version and
  read its result, and the old version's own results stay hidden. Rejected.
- **Also hide finished Runs but not running ones.** Two rules for one question, and a person
  cannot tell which applies to a Run they open. Rejected.

## Consequences

- A restriction narrows who can use a version (ADR 0095). It no longer narrows who can read what
  a Run that used it produced.
- Hiding results means denying `registry.read` on the version in the Tenant's policy. That hides
  the outputs of its Runs and of every Run that read them, and stops those Runs, as a policy
  denial is a read check like any other. The dashboard has no control for it; the withdrawal dialog
  says how.
- After a withdrawal, what a Run already received stays readable to holders of `registry.read`,
  though they cannot open the definition directly (`GET /api/registry` and what discovery offers
  still need the approval).
- Resuming a Run whose Binding names a withdrawn version answers `409` with a reason, naming the
  version only to a holder of `registry.read` on it, and does not wake the Run to pause it again.
- An Agent found on a Node of the reader's own Tenant is read by `registry.read` and
  `federation.discover` on that Node, without asking it, so deleting the Node or its not
  answering hides nothing. An Agent found through a Peer Mapping is still verified with its Node,
  which no longer looks at the approval, so revoking the mapping or its credential still hides the
  outputs.
- ADR 0095 is not contradicted: its restriction still names the direct user for use. Its
  consequences say nothing of readers, so it needs no amendment.
- Egress's per-request check of the Agent's own approval is unchanged. A withdrawn dependency
  therefore stops a running command at the next recheck of the command, not at its next request.
