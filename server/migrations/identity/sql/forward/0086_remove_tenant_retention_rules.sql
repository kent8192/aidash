-- A Tenant's Retention Rules set how long Document snapshots were kept, and
-- Research no longer keeps any (#987, ADR 0138). A typed DropTable cannot drop
-- the row policies, so the drop and its reversal are SQL.
DROP TABLE public.tenant_retention_rule_history, public.tenant_retention_rules;
