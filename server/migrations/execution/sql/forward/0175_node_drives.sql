-- Reinhardt cannot define a trigger or a set-based seed from another table.
SET LOCAL aidash.scope = 'deployment';
INSERT INTO public.drives (id, tenant, node, closed, head)
SELECT gen_random_uuid(), tenant, name, false, 0 FROM public.nodes
WHERE NOT EXISTS (
    SELECT 1 FROM public.node_deletions d
    WHERE d.tenant = nodes.tenant AND d.node = nodes.name AND d.purged_at IS NOT NULL
);
CREATE FUNCTION public.aidash_create_node_drive() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog, public AS $$
BEGIN
    INSERT INTO public.drives (id, tenant, node, closed, head)
    VALUES (gen_random_uuid(), NEW.tenant, NEW.name, false, 0);
    RETURN NEW;
END
$$;
CREATE TRIGGER aidash_create_node_drive
AFTER INSERT ON public.nodes FOR EACH ROW EXECUTE FUNCTION public.aidash_create_node_drive();
