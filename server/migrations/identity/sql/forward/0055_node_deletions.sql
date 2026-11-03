-- Deletion identity and its deadline are immutable; restore closes this row
-- and another deletion gets a fresh row. Progress updates cannot extend grace.
CREATE FUNCTION public.aidash_node_deletion_immutable() RETURNS trigger
    LANGUAGE plpgsql SET search_path = pg_catalog AS $node_deletion$
BEGIN
    IF (NEW.id, NEW.tenant, NEW.node, NEW.deleted_at, NEW.deleted_by_kind,
        NEW.deleted_by_identity, NEW.purge_after) IS DISTINCT FROM
       (OLD.id, OLD.tenant, OLD.node, OLD.deleted_at, OLD.deleted_by_kind,
        OLD.deleted_by_identity, OLD.purge_after) THEN
        RAISE EXCEPTION 'Node deletion identity and deadline are immutable'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END
$node_deletion$;
CREATE TRIGGER node_deletions_immutable BEFORE UPDATE ON public.node_deletions
    FOR EACH ROW EXECUTE FUNCTION public.aidash_node_deletion_immutable();
