-- The Tenant Duty limits are gone (#1012): no count or minimum interval bounds
-- a Duty; a Budget bounds what it spends. The table carries a forced row-level
-- policy, which a typed DropTable cannot drop or restore.
DROP POLICY tenant_isolation ON public.schedule_limits;
DROP TABLE public.schedule_limits;
