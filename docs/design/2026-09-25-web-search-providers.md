# Web-search provider evaluation

Checked: 2026-09-25
Related: [Issue #44 design interview](2026-09-25-web-search.md)
Status: Brave Web Search selected conditionally by acceptance of Q10 on 2026-09-25. Actual contract eligibility and price remain unverified. No account provisioned, paid request issued, or search-quality benchmark performed.

## Published capabilities and costs

Prices below are USD list prices, before discounts, credits, taxes, and any additional content-processing or model charges. They are not a quote for an Aidash deployment.

| Candidate | Search and source access | Published usage price | Documented operational controls |
| --- | --- | --- | --- |
| Brave Search | Web Search returns URLs, snippets and metadata; LLM Context returns extracted passages. An arbitrary-URL page reader needs a separately chosen retrieval path. | Search: $5 per 1,000 requests. | Search plan lists 50 queries/second. Web Search supports country/language and freshness controls; token authentication is in a header. |
| Tavily | Separate Search and Extract endpoints; extraction reports successful and failed URLs independently. Search supports domain, date, country and language controls. | PAYGO: $0.008/credit. Basic Search costs 1 credit, Advanced Search 2. Basic Extract costs 1 credit per 5 successful URLs; Advanced Extract 2. | Documented development/production limits are 100/1,000 requests/minute for these endpoints. Bearer authentication. Explicit search depth avoids automatic escalation to more expensive search. |
| Exa | Search and Contents endpoints provide source URLs and page content. Contents can request fresh retrieval or cached content through `maxAgeHours`; cached content must not be presented as freshly fetched. | Standard Search: $7 per 1,000 requests with up to 10 results. Contents: $1 per 1,000 pages per content type; summaries and additional results can add charges. | Billing docs list 10 QPS as the default Search limit, with account-specific increases. Contents uses URL-based retrieval and reports per-URL outcomes. |

Sources: [Brave pricing](https://brave.com/search/api/), [Brave Web Search schema](https://api-dashboard.search.brave.com/api-reference/web/search/post), [Brave LLM Context](https://api-dashboard.search.brave.com/documentation/services/llm-context), [Tavily pricing](https://docs.tavily.com/documentation/api-credits), [Tavily Search](https://docs.tavily.com/documentation/api-reference/endpoint/search), [Tavily Extract](https://docs.tavily.com/documentation/api-reference/endpoint/extract), [Tavily rate limits](https://docs.tavily.com/documentation/rate-limits), [Exa pricing](https://exa.ai/docs/admin/pricing), [Exa Contents](https://exa.ai/docs/reference/contents), [Exa billing](https://exa.ai/docs/admin/billing).

## Data handling and durable history

Brave's API FAQ says storing API results requires a plan with explicit storage rights. Its API privacy notice states that search-query records may be retained for up to 90 days for billing/troubleshooting and describes an Enterprise zero-retention option. Therefore neither permission to retain durable result excerpts nor zero retention should be inferred from the ordinary Search price. This directly affects Aidash's Run history and recovery requirements. Sources: [API FAQ](https://brave.com/search/api/), [API privacy notice](https://api-dashboard.search.brave.com/privacy-policy).

Tavily's privacy policy permits use of some query data to improve future responses unless the customer contract specifies otherwise. Its platform terms also contain training provisions for AI Functionality; their application to the exact selected endpoints and account terms must be established rather than inferred from a marketing zero-retention statement. A fixed default API-query retention period and permission for Aidash's proposed durable result retention have not been verified. Sources: [privacy policy](https://www.tavily.com/privacy), [platform terms](https://www.tavily.com/terms).

Exa's pricing documentation offers Zero Data Retention through Enterprise. The inspected general service terms grant broad rights to use inputs and outputs for product development and improvement; those terms do not establish the required no-training commitment for an ordinary account. Default query-retention duration and compatibility with Aidash's proposed result retention remain unverified. No general website privacy statement is treated as an account-specific API guarantee. Sources: [Exa pricing](https://exa.ai/docs/admin/pricing), [Exa service terms](https://exa.ai/terms-of-service).

Perplexity was additionally screened because its general API material describes restrictive data handling. Its Search-specific addendum instead permits broad use of Search Data for service development and says it overrides the general agreement for Search; the inspected official localized text does not establish the accepted no-training requirement. Therefore a generic API zero-retention statement is insufficient to qualify its standalone Search endpoint. Source: [official Search addendum](https://www.perplexity.ai/zh-TW/hub/legal/perplexity-api-terms-of-service-search). No live evaluation was run.

The source publisher's access restrictions remain distinct from the search provider's API terms. Search availability does not prove that the complete source can be retrieved. Q6 now requires bounded durable evidence under the Run retention policy; Q9 allows bounded operational query retention, prohibits model-training reuse, and does not require Japan-only processing or zero retention.

## Selected architecture and deployment conditions

Select Brave Web Search as the first provider integration, paired with an Aidash-controlled reader for public HTML, plain text and text-based PDFs. Its documented geographic/language/freshness controls fit Q2, and direct reading provides an explicit boundary for page authorization, retrieval time and evidence extraction. This is an architectural-fit decision, not a claim of superior measured relevance. See [ADR-0003](../adr/0003-brave-search-and-direct-source-reading.md).

Provider eligibility remains conditional on the actual account/plan explicitly supporting durable result retention and the accepted no-training requirement. Verify pricing, permitted integration evaluation, retention conditions and any termination/deletion obligations as part of that eligibility. A generic boolean or an API key alone is not evidence that those requirements are met. The ordinary public Search price is not a verified quote for a storage-rights plan. Do not weaken Q6/Q9, raise the accepted budget, or enable the service while eligibility is unresolved.

Use the provider's search results as candidate sources and the separately fetched page as the evidence examined by the Agent. Do not substitute a provider-generated answer for Aidash's existing model. No automatic secondary-provider fallback is proposed, because it changes disclosure and account-cost conditions. Alternatives remain documented for a later deliberate provider change or an inability to satisfy Brave's eligibility within the deployment budget.

## Evidence needed before final selection

- Confirm the selected account/plan's result-storage rights and query-retention/training conditions against the settled product requirements.
- Compare a fixed set of public Japanese and English queries covering technical documentation, general research, and recent information. Judge relevant primary sources in the top results, fresh information, extractability, metadata fidelity, latency and metered cost separately.
- Use the same query set and result limits for each measured candidate. Record endpoint/options, date, sample size and failures. Provider marketing benchmarks are not Aidash acceptance evidence.
- Test empty results, 429, authentication failures, provider outages, partial page retrieval, bounds and cancellation through deterministic fixtures. A successful fixture run does not prove live provider availability or relevance.
- Document configured versus observed provider limits; never treat a public list limit as the entitlement of a particular account.

The integration target is selected; its operational eligibility remains unverified. Contract questions that cannot be verified from public material remain explicit deployment prerequisites. The [consolidated specification](2026-09-25-web-search-specification.md) defines the contract, controls and acceptance evidence.
