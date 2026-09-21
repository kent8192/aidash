# Record integrity constraints

`m20260921_071045_record_constraints` adds database enforcement for invariants
already required by registration and execution. Existing primary keys, unique
keys, status checks and foreign keys remain in place.

| Records                                     | Added enforcement                                                                                                                                            |
| ------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Registry entities                           | Identifier syntax, SemVer, typed JSON identity matching the key/kind columns, metadata/config/schema and list shapes                                         |
| Models                                      | OpenRouter provider, nonblank model ID/endpoint, integral context window of at least 2048, text modality, supported reasoning effort or omitted/null default |
| Agents and skills                           | Nonblank instructions; agent step limit is an integer in 1–1000 (omission retains the default)                                                               |
| Workspaces and tasks                        | Nonblank content, object state/requirements, nonnegative revisions                                                                                           |
| Tasks                                       | No direct self-parent or self-dependency, no null dependency elements, parent in the same workspace                                                          |
| Artifacts and generation requests           | Referenced task belongs to the record's workspace                                                                                                            |
| Runs and human requests                     | Nonnegative step/revision, paired lease owner/expiry, human request belongs to its run's workspace                                                           |
| Packages and installations                  | SemVer and manifest identity matching package keys; object installation overrides                                                                            |
| Semantic indexes, entries and reads         | Positive revisions and nonnegative retry attempts                                                                                                            |
| Authorization and generation policy history | Positive revisions                                                                                                                                           |

The API rejects whitespace-only model IDs and skill instructions before database
insertion. Other semantic validation remains in Rust: JSON Schema compilation,
endpoint/credential checks, model and agent reference resolution, dependency
cycles, authorization, token budgets, and lifecycle transitions.

Names and model configuration are intentionally not global unique keys. Aliases,
localized names and distinct immutable versions may share a model configuration;
the registry identity remains `(id, version)`. Remote runs can refer to task and
workspace IDs owned by another node, so there is deliberately no local foreign
key from runs to tasks/workspaces. Historical audit and discovery references also
remain valid after the live target is removed. Existing scoped authorization,
generation budget and transaction checks are retained rather than duplicated.

## Upgrade and rollback

The migration validates existing rows immediately and runs transactionally. It
does not delete, rewrite, normalize or deduplicate existing records. An invalid
row aborts the upgrade with the named constraint; inspect and explicitly correct
the affected data before retrying. Required JSON keys use `COALESCE(..., false)`
so missing keys cannot bypass PostgreSQL CHECK constraints through SQL NULL.

Rollback removes only the new constraints and their supporting indexes. The
integration suite covers direct writes bypassing the API, valid aliases and
versions, remote execution, successful down/up with retained records, and a failed
upgrade followed by correction and retry.

```sh
AIDASH_TEST_DATABASE_URL=postgres://aidash:aidash-local@127.0.0.1:54370/aidash_test \
  cargo test --locked --test record_constraints -- --ignored
```
