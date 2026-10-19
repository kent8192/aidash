# Prompt Cache Scope is separated per Tenant by a keyed salt at the start of the system prompt

Decided on 2026-10-09 (#116, #142, #172). Model calls currently use one node-wide OpenRouter key, so a provider's prompt cache is shared by every Tenant on the Node. A matching prefix does not return another Tenant's content. It does open a timing side channel: a fast response reveals that someone else sent the same prefix, and system and tool prefixes can be identical across Tenants.

From `Ordered` on, the first line of `system` is the Tenant Cache Salt: `aidash-cache-scope:v{version}:{hex}`, where `hex` is HMAC-SHA256 of the Tenant ID under the node-configured Cache Salt Key of that version, truncated to 128 bits. Stable Prefixes therefore never match across Tenants. Within one Tenant, Runs may share cached prefixes. Only the transport adapter computes the salt, immediately before sending; request metadata carries just the Tenant and the key version, so compaction state, digests and diagnostics never contain it. A remote Run is salted with the receiver's local Tenant and the receiver's key, because the receiver's provider account defines the shared cache. A node without a Cache Salt Key rejects Run creation on a salted version instead of sending unsalted requests.

## Considered Options

- Allow explicit `cache_control` only on Tenant-bound Provider Credentials. Rejected for now: BYOK is not implemented, and this would not stop cross-Tenant hits from automatic prefix caching on OpenAI or DeepSeek routes.
- Accept cross-Tenant sharing for prefixes without Tenant data. Rejected: it leaves the timing side channel open.
- Use the raw Tenant ID as the salt. Rejected: it would put a Tenant identifier into prompts and provider logs.
- A provider cache-key parameter. Not relied on: support varies by route, and a salt in the content also works for automatic caching.

## Consequences

- Anthropic's cache order is tools → system → messages. No cache breakpoint may end before `system` does, so no tools-only entry can be shared across Tenants.
- The Cache Salt Key lives in node configuration (`cache_salt.keys` and `cache_salt.current`), not in the Provider Credential Store, because it is not Key Material. It is never logged or returned. Diagnostics and Usage Records show only the key version. Rotating the key only causes cache misses.
- `Legacy` Runs keep their exact bytes and therefore stay unsalted.
- When BYOK lands, the scope may move to the Provider Credential. It never widens beyond one Tenant.
- Each of the following ends the Stable Prefix at the point it changes and changes its cache identity:
  - Load or Unload in the Exposure set (#115);
  - a change to a resident Skill body;
  - a change to instructions, policy or profile.

  Loads and unloads are applied in batches per step, and every identity change is recorded in the Usage Record (#177).
