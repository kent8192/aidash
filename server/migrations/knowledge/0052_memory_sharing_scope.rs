// reinhardt-migration-source: 1
// Reinhardt cannot express PostgreSQL row policies or statement triggers.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0052_memory_sharing_scope", "knowledge")
		.add_dependency("knowledge", "0051_recorded_memory")
		.add_dependency("identity", "0028_tenancy_policy_functions")
		.add_operation(Operation::CreateTable {
			name: "memory_tenant_scope".into(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text).with_primary_key(true).with_not_null(true),
				ColumnDefinition::new("scope", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey { name: "memory_scope_tenant_fkey".into(), columns: vec!["tenant".into()], referenced_table: "tenants".into(), referenced_columns: vec!["name".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::NoAction, deferrable: None },
				Constraint::Check { name: "memory_scope_valid".into(), expression: "scope IN ('workspace','tenant_agent') AND revision > 0".into() },
			], without_rowid: None, interleave_in_parent: None, partition: None,
		})
		.add_operation(Operation::RunSQL {
			sql: "ALTER TABLE public.memory_tenant_scope ENABLE ROW LEVEL SECURITY; ALTER TABLE public.memory_tenant_scope FORCE ROW LEVEL SECURITY; CREATE POLICY tenant_isolation ON public.memory_tenant_scope AS PERMISSIVE FOR ALL TO PUBLIC USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant())) WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant())); CREATE TRIGGER memory_tenant_scope_atomic BEFORE INSERT OR UPDATE OR DELETE ON memory_tenant_scope FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard();".into(),
			reverse_sql: Some("DROP TRIGGER memory_tenant_scope_atomic ON memory_tenant_scope; DROP POLICY tenant_isolation ON public.memory_tenant_scope; ALTER TABLE public.memory_tenant_scope NO FORCE ROW LEVEL SECURITY; ALTER TABLE public.memory_tenant_scope DISABLE ROW LEVEL SECURITY;".into()),
		}).atomic(true)
}
