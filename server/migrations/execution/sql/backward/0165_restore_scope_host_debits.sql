ALTER TABLE public.egress_scope_host_debits
    ADD CONSTRAINT egress_scope_host_debits_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants (name),
    ADD CONSTRAINT egress_scope_host_debits_scope_fkey FOREIGN KEY (tenant, node, scope_id) REFERENCES public.egress_scope_state (tenant, node, scope_id) ON DELETE CASCADE,
    ADD CONSTRAINT egress_scope_host_debits_nonnegative CHECK (bytes >= 0);
CREATE INDEX idx_egress_scope_host_debits_tenant_node_scope_id ON public.egress_scope_host_debits (tenant, node, scope_id);
ALTER TABLE public.egress_scope_host_debits ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.egress_scope_host_debits FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON public.egress_scope_host_debits AS PERMISSIVE FOR ALL TO PUBLIC
    USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()))
    WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));
