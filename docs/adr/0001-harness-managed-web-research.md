---
status: accepted
---

# Provide search and source reading through the Harness

For [Issue #44](https://github.com/kent8192/aidash/issues/44), provide public-Web search and source-page reading as Harness-managed capabilities so Agents can inspect evidence before producing cited answers, without Registry tool records. The node operator owns and pays for the external search-service account, keeping credentials outside Agent context; this favors centrally governed usage over per-user API-key management. Search-service submissions contain search terms and search conditions rather than automatically forwarded conversations or files, and nonpublic information is not disclosed by default; [ADR-0002](0002-web-evidence-and-query-disclosure.md) records the resulting disclosure and evidence boundaries.
