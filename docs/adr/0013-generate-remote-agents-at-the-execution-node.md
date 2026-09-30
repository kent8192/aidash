---
status: accepted
---

# Generate remote Agents under the execution node's policy

For [Issue #75](https://github.com/kent8192/aidash/issues/75), an Agent needed for a Home-node Task is generated at its execution node under that node's generation policy, with a durable binding back to the Home Task and subsequent remote grant. This keeps the Agent definition, generation lifecycle and local resource allowances with the operator responsible for running it, instead of copying a Home-generated definition and assuming its authority follows. Generated delegators at Home still constrain the assignment and its provider usage; generation is preparation, and activation requires the exact bound remote grant and receiver admission.

The [specification](../design/2026-09-30-remote-semantic-memory-specification.md) describes the preparation sequence needed to avoid making generation depend on a grant for an Agent that does not exist yet.
