use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const TABLES: &[&str] = &[
	"marketplace_gate",
	"marketplace_versions",
	"marketplace_audiences",
	"marketplace_consents",
	"marketplace_installations",
	"marketplace_revisions",
	"marketplace_requests",
	"marketplace_provenance",
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		// Documents are versioned application contracts. Indexed identities remain
		// separate columns, with unique constraints independent of JSON encoding.
		for name in TABLES {
			let mut table = Table::create();
			table
				.table(Alias::new(*name))
				.col(ColumnDef::new(Alias::new("key")).text().primary_key())
				.col(
					ColumnDef::new(Alias::new("document"))
						.json_binary()
						.not_null(),
				);
			if *name == "marketplace_versions" {
				for column in [
					"repository",
					"owner",
					"package_id",
					"version",
					"kind",
					"source_id",
					"source_version",
					"source_content",
				] {
					table.col(ColumnDef::new(Alias::new(column)).text().not_null());
				}
				table.index(
					Index::create()
						.unique()
						.col(Alias::new("repository"))
						.col(Alias::new("owner"))
						.col(Alias::new("package_id"))
						.col(Alias::new("version")),
				);
			}
			if *name == "marketplace_installations" {
				table
					.col(ColumnDef::new(Alias::new("tenant")).text().not_null())
					.col(ColumnDef::new(Alias::new("package_key")).text().not_null())
					.index(
						Index::create()
							.unique()
							.col(Alias::new("tenant"))
							.col(Alias::new("package_key")),
					);
			}
			if *name == "marketplace_revisions" {
				table
					.col(ColumnDef::new(Alias::new("installation")).text().not_null())
					.col(
						ColumnDef::new(Alias::new("revision"))
							.big_integer()
							.not_null(),
					)
					.col(ColumnDef::new(Alias::new("entry_id")).text().not_null())
					.col(
						ColumnDef::new(Alias::new("entry_version"))
							.text()
							.not_null(),
					)
					.index(
						Index::create()
							.unique()
							.col(Alias::new("installation"))
							.col(Alias::new("revision")),
					)
					.index(
						Index::create()
							.unique()
							.col(Alias::new("entry_id"))
							.col(Alias::new("entry_version")),
					)
					.foreign_key(
						ForeignKey::create()
							.from(Alias::new(*name), Alias::new("installation"))
							.to(Alias::new("marketplace_installations"), Alias::new("key")),
					)
					.foreign_key(
						ForeignKey::create()
							.from(
								Alias::new(*name),
								(Alias::new("entry_id"), Alias::new("entry_version")),
							)
							.to(
								Alias::new("registry"),
								(Alias::new("id"), Alias::new("version")),
							),
					);
			}
			manager.create_table(table.to_owned()).await?;
			// SeaQuery cannot express PostgreSQL trigger definitions or attachments.
			manager.get_connection().execute_unprepared(&format!("CREATE TRIGGER atomic_write_guard BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON {name} FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard()" )).await?;
		}
		manager
			.create_index(
				Index::create()
					.name("marketplace_versions_source_content")
					.table(Alias::new("marketplace_versions"))
					.col(Alias::new("owner"))
					.col(Alias::new("source_id"))
					.col(Alias::new("source_version"))
					.col(Alias::new("source_content"))
					.to_owned(),
			)
			.await?;
		for (name, field) in [
			("events_marketplace_tenant_sequence", "tenant"),
			("events_marketplace_package_sequence", "key"),
		] {
			manager
				.create_index(
					Index::create()
						.name(name)
						.table(Alias::new("events"))
						.col(Expr::cust(format!("data->>'{field}'")))
						.col(Alias::new("sequence"))
						.and_where(Expr::col(Alias::new("workspace_id")).is_null())
						.and_where(Expr::col(Alias::new("kind")).like("marketplace.%"))
						.and_where(Expr::col(Alias::new("kind")).ne("marketplace.audit"))
						.to_owned(),
				)
				.await?;
		}
		manager
			.create_index(
				Index::create()
					.name("marketplace_audiences_tenants")
					.table(Alias::new("marketplace_audiences"))
					.col(Expr::cust("document->'tenants'"))
					.index_type(IndexType::Custom(Alias::new("gin").into_iden()))
					.to_owned(),
			)
			.await?;
		manager
			.get_connection()
			.execute(
				manager.get_database_backend().build(
					Query::insert()
						.into_table(Alias::new("marketplace_gate"))
						.columns([Alias::new("key"), Alias::new("document")])
						.values_panic([
							"v1".into(),
							serde_json::json!({"enabled":false,"revision":1,"contract":1}).into(),
						]),
				),
			)
			.await?;
		// SeaQuery has no PL/pgSQL/trigger API. Protect marked projection rows even
		// from a previous binary that does not know the tenant/revision contract.
		metadata_shape(manager, true).await?;
		manager.get_connection().execute_unprepared(GUARDS).await?;
		Ok(())
	}
	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		// Refuse a downgrade that would turn tenant projections into native rows.
		manager
			.get_connection()
			.execute_unprepared(
				r#"DO $$ BEGIN
			IF EXISTS (SELECT 1 FROM marketplace_revisions) OR EXISTS (SELECT 1 FROM marketplace_versions) THEN
				RAISE EXCEPTION 'retain Marketplace content; restore a compatible binary';
			END IF;
		END $$;
		DROP TRIGGER marketplace_registry_fence ON registry;
		DROP TRIGGER marketplace_overlay_fence ON installations;
		DROP TRIGGER marketplace_catalog_fence ON authorization_catalog;
		DROP FUNCTION marketplace_registry_fence();
		DROP FUNCTION marketplace_overlay_fence();
		DROP FUNCTION marketplace_catalog_fence();"#,
			)
			.await?;
		metadata_shape(manager, false).await?;
		for name in [
			"events_marketplace_tenant_sequence",
			"events_marketplace_package_sequence",
		] {
			manager
				.drop_index(
					Index::drop()
						.name(name)
						.table(Alias::new("events"))
						.to_owned(),
				)
				.await?;
		}
		for name in TABLES.iter().rev() {
			manager
				.drop_table(Table::drop().table(Alias::new(*name)).to_owned())
				.await?;
		}
		manager
			.get_connection()
			.execute_unprepared("DROP FUNCTION marketplace_immutable()")
			.await?;
		Ok(())
	}
}

const GUARDS: &str = r#"
CREATE FUNCTION marketplace_immutable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'Marketplace content is immutable'; END $$;
CREATE TRIGGER marketplace_immutable BEFORE UPDATE OR DELETE ON marketplace_versions FOR EACH ROW EXECUTE FUNCTION marketplace_immutable();
CREATE TRIGGER marketplace_immutable BEFORE UPDATE OR DELETE ON marketplace_revisions FOR EACH ROW EXECUTE FUNCTION marketplace_immutable();
CREATE FUNCTION marketplace_registry_fence() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.metadata ? 'installation' OR (TG_OP='UPDATE' AND OLD.metadata ? 'installation') OR EXISTS (SELECT 1 FROM registry WHERE id=NEW.id AND version=NEW.version AND metadata ? 'installation') THEN
  IF TG_OP <> 'INSERT' OR current_setting('aidash.marketplace_writer', true) IS DISTINCT FROM '1'
    OR NEW.metadata->'installation'->>'contract' IS DISTINCT FROM '1'
    OR NEW.id NOT LIKE 'mkt-%' THEN
   RAISE EXCEPTION 'unsupported Marketplace writer';
  END IF;
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER marketplace_registry_fence BEFORE INSERT OR UPDATE ON registry FOR EACH ROW EXECUTE FUNCTION marketplace_registry_fence();
CREATE FUNCTION marketplace_overlay_fence() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF EXISTS (SELECT 1 FROM registry WHERE id=NEW.id AND version=NEW.version AND metadata ? 'installation') THEN RAISE EXCEPTION 'Marketplace projections cannot have legacy overlays'; END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER marketplace_overlay_fence BEFORE INSERT OR UPDATE ON installations FOR EACH ROW EXECUTE FUNCTION marketplace_overlay_fence();
CREATE FUNCTION marketplace_catalog_fence() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE owner_tenant text;
BEGIN
 SELECT metadata->'installation'->>'tenant' INTO owner_tenant FROM registry WHERE id=NEW.entry_id AND version=NEW.entry_version;
 IF owner_tenant IS NOT NULL AND (owner_tenant <> NEW.tenant OR current_setting('aidash.marketplace_writer',true) IS DISTINCT FROM '1') THEN
  RAISE EXCEPTION 'unsupported Marketplace catalog writer';
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER marketplace_catalog_fence BEFORE INSERT OR UPDATE ON authorization_catalog FOR EACH ROW EXECUTE FUNCTION marketplace_catalog_fence();
"#;

// ADD/DROP CHECK on an existing table is not expressible by SeaQuery 0.32.
// Reuse the historical shape, adding exactly the server-owned projection field.
async fn metadata_shape(manager: &SchemaManager<'_>, projections: bool) -> Result<(), DbErr> {
	let (_, _, mut expression) = crate::m20260921_071045_record_constraints::workbench_checks()
		.into_iter()
		.find(|(_, name, _)| *name == "registry_metadata_shape")
		.expect("registry shape");
	if projections {
		expression = expression.replace("metadata", "(metadata - 'installation')");
		expression.push_str(" AND (NOT (metadata ? 'installation') OR (jsonb_typeof(metadata->'installation')='object' AND metadata->'installation'->>'contract'='1' AND jsonb_typeof(metadata->'installation'->'tenant')='string' AND jsonb_typeof(metadata->'installation'->'installation')='string' AND jsonb_typeof(metadata->'installation'->'revision')='number'))");
	}
	manager.get_connection().execute_unprepared(&format!("ALTER TABLE registry DROP CONSTRAINT registry_metadata_shape; ALTER TABLE registry ADD CONSTRAINT registry_metadata_shape CHECK ({expression})")).await?;
	Ok(())
}
