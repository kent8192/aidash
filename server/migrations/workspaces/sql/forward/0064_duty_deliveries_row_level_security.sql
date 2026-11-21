ALTER TABLE public.duty_deliveries ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.duty_deliveries FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.duty_deliveries AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
