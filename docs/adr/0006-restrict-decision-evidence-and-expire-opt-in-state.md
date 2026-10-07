---
status: accepted
---

# Restrict decision evidence and expire opt-in state

Detailed evidence requires Run access, dedicated decision-evidence authority, and applicable source restrictions, while ordinary events expose only a safe summary. Full Decision state recording requires explicit tenant-authorized Node policy before evaluation, defaults to seven days, and has a thirty-day ceiling subject to stricter source constraints; the state digest identifies the exact permitted input after mandatory redaction. This keeps an operational audit trail while accepting that full provider-input experiments become unavailable after state expiry or source revocation.

Compaction state uses an explicit disclosure allowlist instead of copying full Agent or Skill instructions, private documents, or raw tool-result bodies. Inherited source restrictions apply to derived text, and undisclosable or provenance-unknown events remain retained rather than becoming probabilistic pruning targets.
