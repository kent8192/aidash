# OpenRouter Provider tools

An Agent can bind the Deployment's standard Provider tools:
`provider.openrouter.web_search`, `provider.openrouter.web_fetch`,
`provider.openrouter.datetime` and `provider.openrouter.fusion`, version `1.0.0`.
Each definition and its one-tool bundle require Catalog Approval. A Tenant cannot
replace these definitions. The approval is the only consent; Provider tools do
not use runtime Approval Rules or Web disclosure approvals.

Only Run inference on OpenRouter with the Tenant's OpenRouter Provider Credential
may send them. Plan Connections, Keyless calls, maintenance, Deciders and
compaction do not carry them. The application admission seam in
`aidash_application::provider_tools` connects Data Protection (#542); its effective
Standard-only rule is integrated by that slice. Data Region admission belongs to
#543. Provider tool support does not imply that model ZDR covers tool backends.

Agent Bindings pin parameters and spending caps alongside their restrictions:

```json
{
  "kind": "tool",
  "target": {
    "registry_deployment": "aidash://example",
    "id": "provider.openrouter.web_search",
    "version": "1.0.0"
  },
  "narrow": {
    "provider_tool": {
      "max_tool_calls": 3,
      "parameters": {
        "engine": "exa",
        "max_results": 5,
        "max_total_results": 15
      }
    }
  }
}
```

The model cannot override these parameters. Deferred Exposure discovers, loads
and unloads each bound capability normally. Only selected tools reach a request.
The strictest bound among selected tools controls the shared outer loop.

| Limit                                         | Aidash maximum |
| --------------------------------------------- | -------------: |
| Outer `max_tool_calls`                        |              8 |
| Search `max_results`                          |             10 |
| Search `max_total_results`                    |             30 |
| Fetch `max_content_tokens`                    |          16384 |
| Fusion `analysis_models`                      |              4 |
| Fusion inner `max_tool_calls`                 |              4 |
| Fusion `max_completion_tokens` per inner call |           8192 |

Search pins Exa by default; fetch pins the OpenRouter fetcher. Automatic/native
engine selection and Firecrawl are excluded from this version's closed schema.
Fusion requires an explicit panel and analyst; it never relies on an upstream
panel default. Domain filters may also be pinned.

The Capability Token signs each type, the canonical parameter digest and the
outer call bound. The broker and Self-hosted transport apply the same domain
check, including Aidash's hard limits. Unknown provider-owned tools, plugin
controls, presets and stop conditions that override the signed cap are refused.
Old tokens omit the new authority and cannot send any Provider tool. Legacy `:online` suffixes and preset model controls are refused, because they can add provider-owned tools after admission without the signed Binding caps.

Observable activity and annotations remain in the Accepted Response and Context
Journal and are replayed as model-authored history, one Model Turn at a time.
Thread inspection shows them read-only from paged Context Journal entries, so compaction does not hide them. Results above 64 KiB have a bounded preview in inspection; the original journal entry remains complete. Provider-reported usage counts are stored
on the Inference Attempt's Usage Record alongside OpenRouter's exact total cost;
the Run-linked Usage Records remain the cost ledger. No separate tool Request
Estimate or duplicate cost charge is manufactured. URLs, snippets and Fusion
results never create Research Sources, Evidence fragments or Source citations.

## OpenRouter references

Verified on 2026-10-27:

- [Server tools](https://openrouter.ai/docs/guides/features/server-tools): tool entry shape, shared outer loop, usage and privacy boundaries.
- [Web search](https://openrouter.ai/docs/guides/features/server-tools/web-search): parameters, both transports and `url_citation` annotations.
- [Web fetch](https://openrouter.ai/docs/guides/features/server-tools/web-fetch): engines, domain filters, content cap and returned page data.
- [Datetime](https://openrouter.ai/docs/guides/features/server-tools/datetime): timezone and datetime result.
- [Fusion](https://openrouter.ai/docs/guides/features/server-tools/fusion): both transports, explicit panel/analyst and separate inner budgets. Request-level provider preferences do not propagate into its inner generations.

The tools are beta. Activity parsing retains observable supported items and
annotations, never private reasoning. No paid live call is part of local tests.
