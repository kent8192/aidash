# Verification Evidence binds to mounted content, and Checks never change the Working Area

Decided on 2026-10-13 for #112. A Completion Candidate is identified by its Run, its Completion Policy and the digest of every file mounted in the Run's Working Area (scope, path and file digest, across Working, References and Received), never by its text. Verification Evidence counts only for the same Run, Check, capability version, fixed arguments and content digest. Only Checks that Aidash itself invokes at the Completion Gate produce evidence. A Check runs on that exact content in an Execution Pod whose file changes are discarded, so verifying never changes the Working Area. A Check keeps its capability's real effect classification and approval requirements.

## Considered Options

- Bind evidence to the Area revision, as #112 first proposed. Rejected: every publication increments the revision, even when no file changed, and every shell operation publishes. Evidence would invalidate itself when its own Check ran.
- Let Checks publish like ordinary shell operations. Rejected: tests write build output. The runner exports all of `/work` within a 4096-file and `working_bytes` ceiling, so a build directory alone can fail the quota, and any write changes the content under the evidence.
- Include the final text in the candidate's identity. Rejected: Checks test files, not wording. A reworded answer over unchanged content reuses the evidence.
- Also accept model-initiated runs of the same capability as evidence. Rejected for now: deciding which model call matched the declared Check adds a surface for little gain. The model may still run tests for its own feedback; those runs never count.

## Consequences

- The runner gains a verification operation that does not export `/work`. Reference extraction is the existing precedent for a caller that never publishes.
- The gate runs before `begin_final_completion`. New input during verification is accepted and invalidates the candidate. A Check already running finishes, and its evidence is reused if the content is unchanged.
- No candidate is formed while one of the Run's operations is running or uncertain. A Received-scope write during verification invalidates the evidence, and the Check runs again without spending the Repair Allowance.
- The gate recomputes the content digest when it commits completion.
