# OpenRouter Provider tools

An Agent can bind the Deployment's standard Provider tools:
`provider.openrouter.web_search`, `provider.openrouter.web_fetch`,
`provider.openrouter.datetime` and `provider.openrouter.fusion`, version `1.0.0`.
Each definition and its one-tool bundle require Catalog Approval. A Tenant cannot
replace these definitions. The approval is the only consent; Provider tools do
not use runtime Approval Rules or Web disclosure approvals.

Only Run inference on OpenRouter with the Tenant's OpenRouter Provider Credential
may send them. Plan Connections, Keyless calls, maintenance, Deciders and
compaction do not carry them. Immediately before each send, the transport reloads
the current Tenant and delegation-chain Data Protection policy and evaluates
`DataProtection::permits(Destination::ProviderTool)`. No Training and Zero Data
Retention refuse Provider tools before token issuance or provider I/O. This policy
is never pinned in a Binding. Data Region admission belongs to #543. Provider tool support does not imply that model ZDR covers tool backends.

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

Registry migration `0050_provider_tools` extends the installed Tool descriptor and
Agent Binding validation functions while preserving the earlier contracts and
their function identities. PostgreSQL function bodies require DDL through
Reinhardt's `RunSQL`; no application data is rewritten. Downgrade is refused while
immutable definitions or retained Run snapshots require Provider tools. Execution
migration `0061_provider_tool_usage` stores reported tool counts as nullable JSONB.

## OpenRouter references

Verified on 2026-10-27:

- [Server tools](https://openrouter.ai/docs/guides/features/server-tools): tool entry shape, shared outer loop, usage and privacy boundaries.
- [Web search](https://openrouter.ai/docs/guides/features/server-tools/web-search): parameters, both transports and `url_citation` annotations.
- [Web fetch](https://openrouter.ai/docs/guides/features/server-tools/web-fetch): engines, domain filters, content cap and returned page data.
- [Datetime](https://openrouter.ai/docs/guides/features/server-tools/datetime): timezone and datetime result.
- [Fusion](https://openrouter.ai/docs/guides/features/server-tools/fusion): both transports, explicit panel/analyst and separate inner budgets. Request-level provider preferences do not propagate into its inner generations.

The tools are beta. Activity parsing retains observable supported items and
annotations, never private reasoning. No paid live call is part of local tests.

## Warning-tier tools

Version `1.0.0` also offers `provider.openrouter.advisor`,
`provider.openrouter.subagent`, `provider.openrouter.experimental__search_models`,
`provider.openrouter.tool_search` and `provider.openrouter.shell` (one-tool
bundles use the aliases `provider_advisor`, `provider_subagent`,
`provider_search_models`, `provider_tool_search`, `provider_shell`). This first
version requires `wire_api: responses` for these tools. Although OpenRouter also
supports advisor, subagent and model search on Chat Completions, Aidash's Chat
adapter does not receive their complete paired server transcript for faithful
replay. No Messages transport is added.

Catalog Approval displays the execution, delegation, retention and spending
conditions and requires an explicit acknowledgment for each exact warning tool
version, including every warning tool in a dependency closure. The exact English
disclosure is saved on the approval history revision in the same transaction;
withdrawal needs no acknowledgment. Legacy mutation paths cannot approve a warning
tool without it. No runtime consent or repeated execution warning is added.
These tools run outside the Execution Pod, gVisor, Egress gateway and Aidash's
Tenant Secret restrictions. Aidash does not provision Tenant Secrets to them.
Child models run outside Aidash's delegation chain and per-agent budgets.

Advisor and subagent require a pinned child model and completion budget (maximum
8192 tokens, including reasoning). Advisor pins `stream: false` and an explicit
`forward_transcript` boolean. A request carries at most one unnamed advisor,
always at index zero, so replayed `instance_name` retains its positional identity.
Subagent additionally pins an inner call budget (maximum four) and `tools: []`, with `inherit_functions: false`;
nested server tools and experimental client function inheritance are excluded.
OpenRouter includes child usage in the request's reported cost; Aidash does not
charge it a second time. Search models and tool search pin `max_results <= 10`.
Tool search defers only the already admitted function definitions in that request;
it cannot discover an unbound or unloaded Aidash capability. The separate
`deferred_tools` encrypted-state protocol remains disallowed.

### Shell lifetime and spending

Shell pins `engine: openrouter` and `environment.type: container_reference`.
Before the first send, an ORM transaction locks the Run and records its custom
container id and network policy. The remote container is created lazily by
OpenRouter, and later sends and worker recovery reuse the durable pin. The
network policy defaults to `disabled`; a Binding may instead pin at most 32
exact lowercase hostnames. Wildcards, unrestricted egress, local/native engines,
caller-selected container ids and attached workspace files are refused. The
policy cannot change during a Run. Ended Runs cannot initiate another shell send.

**Aidash does not delete containers.** OpenRouter exposes no container deletion
API. It sleeps a container after five idle minutes and keeps home-directory files
up to 30 days after its last use. Open processes, environment variables and system
state do not survive sleep. The durable pin is retained when a Run completes,
fails or is cancelled; recovery does not pretend that remote deletion happened.
This replaces the earlier design decision to delete a container at Run end.

The spending bound combines the signed outer `max_tool_calls` (Aidash maximum
eight), OpenRouter's server-enforced clamps (100 commands per call, 300000 ms per
command, 65536 characters per stdout/stderr stream per command), and Aidash's
model request timeout, which applies through response-body consumption. These
shell arguments are generated by the model, not configurable lower Binding caps.
The broker also applies its configured inference deadline. At the checked pricing,
OpenRouter bills sandbox time at $0.0001 per second only while a request runs,
with a 30-second minimum for new or sleeping containers; idle time between
requests is unbilled. The limits and price are upstream beta contracts.

Observable commands, stdout/stderr, exit/timeout outcomes and file citations are
retained as Provider tool activity. OpenRouter returns at most ten recently
changed files per result; other changed files can still persist remotely. Aidash
only lists these files, never downloads, promotes or imports them into the Working
Area. The existing 1 MiB response limit and journal limits still apply to very large
results; exceeding the transport limit fails the request instead of truncating
and accepting a partial tool transcript. Shell is available on OpenRouter's global endpoint only. Regional
admission remains owned by #543.

### Excluded candidates

| Tool                                   | Reason                                                                                                                                                                                                |
| -------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `bash`, including `engine: openrouter` | Messages-only; Aidash does not implement that transport. Client-side bash is also outside the Provider tool execution boundary.                                                                       |
| `apply_patch`                          | OpenRouter validates a patch but requires the client to execute it and return a result. There is no provider-owned patch executor to bind here.                                                       |
| `files`                                | API-key-wide workspace access, indefinite storage and no configurable per-call file count, size or path limits; mandatory Binding bounds cannot be guaranteed.                                        |
| `image_generation`                     | The documented tool has no enforceable image-count parameter; size/options depend on the chosen image model. A bounded image contract is needed before adding it. The outer broker remains text-only. |

Registry migration `0051_provider_warning_tools` extends the previous closed union
without replacing old validation contracts. Identity migration
`0044_provider_warning_acknowledgment` adds the nullable exact disclosure to
approval history; execution migration `0062_provider_container` adds the nullable
Run pin. Existing approvals and Runs acquire neither acknowledgment nor container
implicitly.

Additional official references, checked 2026-10-28:

- [Advisor](https://openrouter.ai/docs/guides/features/server-tools/advisor): inner model, budget and positional replay identity.
- [Subagent](https://openrouter.ai/docs/guides/features/server-tools/subagent): worker budgets, nested tools and experimental inheritance.
- [Model search](https://openrouter.ai/docs/guides/features/server-tools/search-models): experimental type and result bounds; no extra tool charge.
- [Tool search](https://openrouter.ai/docs/guides/features/server-tools/tool-search): Responses deferral, result cap and separate encrypted-state protocol.
- [Shell](https://openrouter.ai/docs/guides/features/server-tools/shell): model-generated arguments, network policy, files and sandbox pricing.
- [Containers](https://openrouter.ai/docs/guides/features/containers): reference ids, sleeping, retention and file endpoints; no delete API.
- [Bash](https://openrouter.ai/docs/guides/features/server-tools/bash), [Apply patch](https://openrouter.ai/docs/guides/features/server-tools/apply-patch), [Files](https://openrouter.ai/docs/guides/features/server-tools/files), [Image generation](https://openrouter.ai/docs/guides/features/server-tools/image-generation): exclusion evidence.
