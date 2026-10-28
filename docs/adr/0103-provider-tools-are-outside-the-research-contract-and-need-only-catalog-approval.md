---
status: accepted
---

# Provider tools are outside the research contract and need only Catalog Approval

Decided on 2026-10-27. OpenRouter now offers server tools (`openrouter:web_search`, `openrouter:fusion`, `openrouter:shell` and others) that it runs itself inside one model request; the deprecated `:online` suffix and web plugin became `openrouter:web_search`. Aidash exposes them as Provider tools: one Built-in definition per tool, each versioned by Aidash and usable only after a Catalog Approval, bound by an Agent definition with parameters that Aidash validates and the Binding snapshot pins, and available only to a Run whose model is called with the Tenant's Provider Credential for OpenRouter. Deciders, compaction and Jev calls never carry one.

Because Aidash neither executes a Provider tool nor sees what it sends before it is sent, a Provider tool is never a Research operation. ADR 0049 limits a Web disclosure approval to what Aidash itself sends; an approval of a query Aidash cannot see would mislead the approver, so a Provider tool asks nobody during a Run. Its Catalog Approval is the only consent. Tools that act beyond retrieval (`advisor`, `subagent`, `tool_search`, `experimental__search_models`, `shell`) are offered too, but their Catalog Approval shows and requires acknowledging what they bypass: the Execution Pod and Egress gateway, the delegation chain and its budgets, and storage at OpenRouter. Four are left out because no warning can make up for what Aidash cannot bound or where the tool would run: `bash`, which OpenRouter offers only on the Messages API that Aidash does not speak (and whose client-side mode would have Aidash run its commands); `apply_patch`, which the caller applies rather than the provider (Aidash's own Sandboxed `apply_patch` covers patches); `files`, which reaches the whole workspace of the API key with no expiry and no count, size or path limits Aidash can enforce; and `image_generation`, which has no parameter that bounds how many images it makes and breaks the text-only output a signed call authorizes.

What a Provider tool returns is never Research evidence. A source it reports cites nothing until the Run reads it through Page reading.

## Considered Options

- **Treat OpenRouter as a Research provider.** Rejected: ADR 0048 requires Aidash to build the exact `search` request, which OpenRouter's model-chosen queries break.
- **A model property, like the `:online` suffix.** Rejected: an Agent has no research mode and a model has no web flag.
- **A per-Run Provider tool approval with Run grants.** Rejected by the maintainer: once approved, a Provider tool should need no further confirmation or warning.
- **Exclude execution tools wholesale.** Rejected by the maintainer in favour of offering them behind an acknowledged warning; only the four above are left out, for the reasons given.

## Consequences

- OpenRouter models gain an explicit wire API (`chat` or `responses`) in their model definition; a Provider tool that needs the Responses API cannot be bound to a `chat` model.
- Provider tool costs are recorded from OpenRouter's reported usage against the Run's cost ledger, without a Request Estimate; only the tool parameters cap them.
- Provider tool calls and results are kept in the Accepted Response, shown read-only in the Thread and carried in later requests as model-authored history.
- A Run gets at most one OpenRouter shell container, reused across its requests. OpenRouter offers no API to delete one, so Aidash does not: the container sleeps after five idle minutes and its files are deleted 30 days after last use, which the warning at Catalog Approval states. Its files are listed, never imported into the Working Area. Sandbox time is billed only while a request runs, so the model's request timeout bounds it.
- Only a Tenant whose Data Protection Level is Standard can use Provider tools (ADR 0104).
- The Cloud credential broker admits only `function` tools today. A call's Capability Token now signs the exact Provider tools it may carry (type and a digest of the pinned parameters), and the broker admits those and nothing else; without a broker the server applies the same check.
- A signed output-token bound cannot bound a Provider tool's cost, so every Provider tool binding must set its caps (`max_tool_calls`, `max_results`, Fusion's panel size and per-call `max_completion_tokens`, shell timeouts), each within a maximum Aidash fixes, and the broker checks them against the token. A Tenant-level Provider tool budget is not part of this decision.
