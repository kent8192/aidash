ALTER TABLE public.egress_usage_daily
    ADD CONSTRAINT egress_usage_daily_tenant_fkey FOREIGN KEY (tenant) REFERENCES public.tenants(name);
