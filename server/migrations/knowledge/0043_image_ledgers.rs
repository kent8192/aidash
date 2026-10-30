// reinhardt-migration-source: 1
// PostgreSQL row-level security policy DDL is not expressible by SchemaExpr.
use reinhardt::db::migrations::{FieldType, prelude::*};

pub(super) fn migration() -> Migration {
	Migration::new("0043_image_ledgers", "knowledge")
		.add_dependency("knowledge", "0042_image_publications")
		.add_operation(Operation::CreateTable {
			name: "semantic_image_ledgers".into(),
			columns: vec![
				ColumnDefinition::new("workspace_id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("slots", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("authorities", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("initialized", FieldType::Boolean).with_not_null(true).with_default(Some("false".into())),
				ColumnDefinition::new("waiting_authority", FieldType::Boolean).with_not_null(true).with_default(Some("false".into())),
				ColumnDefinition::new("next_authority_check", FieldType::TimestampTz).with_not_null(true).with_default(Some("CURRENT_TIMESTAMP".into())),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::RunSQL {
			sql: "ALTER TABLE public.semantic_image_ledgers ENABLE ROW LEVEL SECURITY; ALTER TABLE public.semantic_image_ledgers FORCE ROW LEVEL SECURITY; CREATE POLICY tenant_isolation ON public.semantic_image_ledgers AS PERMISSIVE FOR ALL TO PUBLIC USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant())) WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));".into(),
			reverse_sql: Some("DROP POLICY tenant_isolation ON public.semantic_image_ledgers; ALTER TABLE public.semantic_image_ledgers NO FORCE ROW LEVEL SECURITY; ALTER TABLE public.semantic_image_ledgers DISABLE ROW LEVEL SECURITY;".into()),
		})
		.atomic(true)
}
