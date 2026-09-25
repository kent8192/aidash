# Web-search live pilot protocol

Related: [Issue #44 specification](2026-09-25-web-search-specification.md), acceptance AT20.
Status: Prepared protocol; NOT RUN. No query below is a claim that a source was found or a factual answer verified.

## Preconditions and freeze

Verify the selected account's data/storage/evaluation rights, tariff, credential configuration, budget and isolated reader first. Do not buy or enable a service merely because this protocol exists. Use public-only authorized Runs and the implementation under test. The normal search/page limits, approvals, logging restrictions and budget ledger remain active.

Immediately before the pilot, choose one UTC `EVALUATION_DATE` and substitute its ISO date literally into every dated query below. Record the resulting exact query list, adapter/commit identity, account plan, rate/budget configuration and test operator. This date substitution is the only predefined variable; freeze the manifest before the first request. Necessary query/protocol changes create a new evaluation revision and retain the prior result, rather than rewriting failures.

Execute each Japanese query with language `ja` and country `JP`, and its English counterpart with language `en` and country `US`. Use `count: 5`, `page: 0`, no domain restriction and no implicit additional search. The adapter must record the final provider query and mapped language code. These are two localized product-use cases, not a controlled experiment attributing every difference solely to query language.

## Twenty queries

Each row is two independently graded searches, JA01-JA10 and EN01-EN10. The source criteria guide grading; they are not fabricated expected search results.

| Pair/category | Japanese query | English query | Evidence sought |
| --- | --- | --- | --- |
| 01 technical | Python asyncio TaskGroup 例外 キャンセル 公式ドキュメント | Python asyncio TaskGroup exceptions cancellation official documentation | Maintainer documentation describing TaskGroup exception/cancellation behavior, with relevant version context. |
| 02 technical | JavaScript AbortController fetch 中止 MDN | JavaScript AbortController cancel fetch MDN | Official MDN API documentation explaining cancellation and a source passage that answers the question. |
| 03 technical | WCAG 2.2 フォーカスが隠れない 最低限 達成基準 | WCAG 2.2 Focus Not Obscured Minimum success criterion | W3C normative/understanding material or its identified authorized translation, matched to the named criterion. |
| 04 technical | Kubernetes Namespace すべてのリソース 名前空間 公式 | Kubernetes namespaces resources not in a namespace official documentation | Kubernetes documentation distinguishing namespaced and cluster-scoped resources. |
| 05 general | 気象庁 震度 マグニチュード 違い | Japan Meteorological Agency seismic intensity magnitude difference | Public-agency material distinguishing the concepts, not an unsupported paraphrase. |
| 06 general | JAXA きぼう 日本実験棟 役割 | JAXA Kibo Japanese Experiment Module purpose | JAXA mission or educational material identifying the module and its functions. |
| 07 general | UNESCO 世界遺産 登録 基準 | UNESCO World Heritage selection criteria | UNESCO or an identified national commission's authoritative criteria, with the underlying official source where needed. |
| 08 recent | EVALUATION_DATE 時点 Python 最新 安定版 リリース 公式 | latest stable Python release as of EVALUATION_DATE official | Dated maintainer release evidence that distinguishes stable from prerelease and existed by the cutoff. |
| 09 recent | EVALUATION_DATE 時点 Rust 最新 安定版 リリース 公式 | latest stable Rust release as of EVALUATION_DATE official | Dated official release evidence, not nightly/beta or a later release. |
| 10 recent | EVALUATION_DATE 以前30日 気象庁 報道発表 | Japan Meteorological Agency press releases in the 30 days ending EVALUATION_DATE | A dated official release in the window; absence requires inspection of the official archive, not merely zero search matches. |

For pair 10 only, explicitly pass an inclusive custom freshness range from 29 days before `EVALUATION_DATE` through that date. For pairs 08-09, do not impose a short date range that could exclude the latest stable release simply because it is older. Correctly interpret a release as-of the cutoff rather than using today's current page uncritically.

## Execution and grading

Use at least two Runs to remain within 10 search attempts per Run; retries also consume attempts, so add another Run when necessary. Keep all attempts within the same configured node monthly budget. Open the most relevant candidates within the normal page budget and use the actual citation path. If additional investigation is needed to establish grading truth, record it separately from the tested top-five result and charge/report every request; it cannot improve the original ranking score retroactively.

For each query record the top-five source IDs/URLs/ranks, retrieval outcomes, supported format, fragment references, requested/effective filters, provider date versus fetch/publisher date, relevant primary-source hit (yes/no with reason), citation support, duration, retry/redirect counts and actual or estimated cost. Include failures and empty/filtered results. Review the cited passage itself; a plausible title, top rank or valid reference ID is not a semantic correctness judgment.

Pass criteria proposed for specification review:

- At least 8/10 queries in each language have a relevant authoritative source among the first five results.
- All six recent-information queries establish the correct as-of answer or substantiate the specified absence with dated primary evidence.
- Cited fragments match what the Agent actually received; all emitted evidence references resolve under the requesting user's authority.
- Normal quota, disclosure, timeout and result-size controls remain active. Report observed latency and cost, with no claim of a production percentile from 20 queries.
- HTML, plain-text and text-PDF extraction also pass their controlled format fixtures; search ranking alone is not evidence of reader-format support.

Summarize median and maximum observed search/read durations, successes, failures, ranking/source-read outcomes and estimated-versus-reconciled charges. Do not label the provider globally best from this sample. A failed criterion leaves rollout pending and creates a focused follow-up; it does not justify silently changing accepted privacy, history or spending requirements.

## Result record

Evaluation manifest: NOT CREATED FOR EXECUTION.
Provider calls: NOT RUN.
Query relevance and freshness: NOT MEASURED.
Reader/citation behavior: NOT VERIFIED.
Latency and billed cost: NOT MEASURED.
Rollout decision: PENDING IMPLEMENTATION, ACCOUNT ELIGIBILITY AND EVIDENCE.
