SET LOCAL search_path = public, pg_catalog;

UPDATE "atomic_gate" SET "transaction_id" = NULL WHERE "transaction_id" IS NOT NULL;
DELETE FROM "atomic_authority_attempts";
DELETE FROM "atomic_preflights";
DELETE FROM "atomic_subjects";
DELETE FROM "atomic_history";
DELETE FROM "atomic_votes";
DELETE FROM "atomic_participants";
DELETE FROM "atomic_coordinators";
