-- Extension index methods and NULLS NOT DISTINCT are unsupported by Reinhardt migrations.
CREATE UNIQUE INDEX memory_bank_scope ON memory_banks (home, tenant, workspace_id, participant_id) NULLS NOT DISTINCT;
CREATE INDEX memory_units_full_text ON memory_units USING pgroonga (text);

-- Trigger installation is unsupported by Reinhardt's typed migration operations.
-- The same global visibility fence used by Messages, Artifacts and semantic sources
-- also protects every native memory write, including receipts and participant bindings.
CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON memory_participants FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON memory_banks FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON memory_units FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON memory_history FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON memory_receipts FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON memory_dependencies FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON memory_candidates FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
CREATE TRIGGER atomic_write_guard BEFORE INSERT OR DELETE OR UPDATE OR TRUNCATE ON memory_run_bindings FOR EACH STATEMENT EXECUTE FUNCTION public.atomic_write_guard();
