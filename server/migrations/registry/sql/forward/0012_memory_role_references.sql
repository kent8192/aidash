-- Unsupported DDL: trigger-maintained versioned role edges prevent reference drift.
CREATE FUNCTION public.guard_registry_memory_role_refs() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF pg_trigger_depth() < 2 THEN
  RAISE EXCEPTION 'memory role references are maintained by registry'
   USING ERRCODE = '23514', CONSTRAINT = 'registry_memory_role_reference';
 END IF;
 RETURN NULL;
END $$;
CREATE FUNCTION public.sync_registry_memory_role_refs() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE config jsonb; binding jsonb; role text; expected text;
BEGIN
 IF TG_OP = 'UPDATE' THEN
  DELETE FROM registry_memory_role_refs WHERE source_id = OLD.id AND source_version = OLD.version;
 END IF;
 config := NEW.metadata->'config';
 IF NEW.kind = 'agent' THEN
  IF jsonb_typeof(config->'memory') = 'object' THEN
   INSERT INTO registry_memory_role_refs VALUES (NEW.id, NEW.version, 'memory', 1, 'memory', config#>>'{memory,id}', config#>>'{memory,version}');
  END IF;
  INSERT INTO registry_memory_role_refs
   SELECT NEW.id, NEW.version, 'sources', ordinal::integer, 'source', value->>'id', value->>'version'
   FROM jsonb_array_elements(COALESCE(config->'sources', '[]'::jsonb)) WITH ORDINALITY AS ref(value, ordinal);
 ELSIF NEW.kind = 'memory' AND config ? 'engine' THEN
  FOREACH role IN ARRAY ARRAY['extraction','derivation','reflection','embedding','reranker','tokenizer'] LOOP
   expected := CASE WHEN role IN ('extraction','derivation','reflection') THEN 'model' ELSE role END;
   binding := config->'policy'->role;
   INSERT INTO registry_memory_role_refs VALUES (NEW.id, NEW.version, role, 1, expected, binding->>'id', binding->>'version');
  END LOOP;
 ELSIF NEW.kind = 'source' AND config ? 'scope' THEN
  INSERT INTO registry_memory_role_refs VALUES (NEW.id, NEW.version, 'memory', 1, 'memory', config#>>'{memory,id}', config#>>'{memory,version}');
 ELSIF NEW.kind = 'reranker' AND config->>'provider' = 'model' THEN
  INSERT INTO registry_memory_role_refs VALUES (NEW.id, NEW.version, 'model', 1, 'model', config#>>'{model,id}', config#>>'{model,version}');
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER registry_memory_role_refs_guard BEFORE INSERT OR UPDATE OR DELETE ON registry_memory_role_refs FOR EACH STATEMENT EXECUTE FUNCTION guard_registry_memory_role_refs();
CREATE TRIGGER registry_memory_role_refs_sync AFTER INSERT OR UPDATE ON registry FOR EACH ROW EXECUTE FUNCTION sync_registry_memory_role_refs();
CREATE TRIGGER registry_memory_role_refs_atomic BEFORE INSERT OR UPDATE OR DELETE ON registry_memory_role_refs FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard();
