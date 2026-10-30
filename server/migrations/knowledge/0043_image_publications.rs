// reinhardt-migration-source: 1
// PostgreSQL row-level security policy DDL is not expressible by SchemaExpr.
use reinhardt::db::migrations::{FieldType, prelude::*};

pub(super) fn migration() -> Migration {
	Migration::new("0043_image_publications", "knowledge")
		.add_dependency("knowledge", "0042_working_images_state")
		.add_operation(Operation::CreateTable {
			name: "semantic_image_publications".into(),
			columns: vec![
				ColumnDefinition::new("sequence", FieldType::BigInteger)
					.with_not_null(true).with_primary_key(true).with_auto_increment(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("workspace_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("area_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("generation", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("changes", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("authority", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true).with_default(Some("CURRENT_TIMESTAMP".into())),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "semantic_image_publications_workspace_fkey".into(),
					columns: vec!["tenant".into(), "workspace_id".into()],
					referenced_table: "workspaces".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::Check {
					name: "semantic_image_publications_shape".into(),
					expression: "generation > 0 AND CASE WHEN jsonb_typeof(changes) = 'array' THEN jsonb_array_length(changes) > 0 ELSE false END AND jsonb_typeof(authority) = 'object'".into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::RunSQL {
			sql: "ALTER TABLE public.semantic_image_publications ENABLE ROW LEVEL SECURITY; ALTER TABLE public.semantic_image_publications FORCE ROW LEVEL SECURITY; CREATE POLICY tenant_isolation ON public.semantic_image_publications AS PERMISSIVE FOR ALL TO PUBLIC USING ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant())) WITH CHECK ((SELECT public.aidash_deployment_scope()) OR tenant = (SELECT public.aidash_scope_tenant()));".into(),
			reverse_sql: Some("DROP POLICY tenant_isolation ON public.semantic_image_publications; ALTER TABLE public.semantic_image_publications NO FORCE ROW LEVEL SECURITY; ALTER TABLE public.semantic_image_publications DISABLE ROW LEVEL SECURITY;".into()),
		})
		.atomic(true)
}
