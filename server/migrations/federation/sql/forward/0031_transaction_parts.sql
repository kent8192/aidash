SET LOCAL search_path = public, pg_catalog;

-- From #1107 (ADR 0146) a coordinator seals every part with a per-transaction
-- seed and a participant keeps only its own share. Journals in the former
-- shape carry no seed and hold whole manifests, so no build can read them.
-- Aidash is undeployed and new cross-Deployment submissions were refused
-- while the old shape was current, so they are discarded. Reinhardt
-- migrations have no operation that deletes rows.
UPDATE atomic_gate SET transaction_id = NULL WHERE transaction_id IS NOT NULL;
DELETE FROM atomic_authority_attempts;
DELETE FROM atomic_preflights;
DELETE FROM atomic_subjects;
DELETE FROM atomic_history;
DELETE FROM atomic_votes;
DELETE FROM atomic_participants;
DELETE FROM atomic_coordinators;
