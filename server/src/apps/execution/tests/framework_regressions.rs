//! Consumer regressions for the framework fixes required by this migration.
use reinhardt::db::associations::ForeignKeyField;
use reinhardt::db::migrations::{FilesystemSource, MigrationSource};
use reinhardt::model;
use reinhardt::query::{
	Alias, ColumnDef, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};
use reinhardt::test::fixtures::temp_dir;
use rstest::{fixture, rstest};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tempfile::TempDir;
use uuid::Uuid;

#[path = "support/framework_serializers.rs"]
mod serializers;
use reinhardt::rest::openapi::ToSchema;
use serializers::Metadata;

#[model(app_label = "upgrade_probe", table_name = "upgrade_parents")]
#[derive(Serialize, Deserialize)]
struct Parent {
	#[field(primary_key = true, max_length = 255)]
	id: String,
}

#[model(app_label = "upgrade_probe", table_name = "upgrade_children")]
#[derive(Serialize, Deserialize)]
struct Child {
	#[field(primary_key = true)]
	id: Uuid,
	#[rel(foreign_key, null = true)]
	parent: ForeignKeyField<Parent>,
}

#[fixture]
fn child_id() -> Uuid {
	Uuid::from_u128(1)
}

#[rstest]
#[case::absent(None)]
#[case::text_key(Some("aidash://peer"))]
#[case::unicode_key(Some("aidash://東京"))]
fn nullable_relations_round_trip_without_model_forms(child_id: Uuid, #[case] key: Option<&str>) {
	// Arrange
	let builder = Child::build().id(child_id);
	// Act
	let child = match key {
		Some(key) => builder.parent(key.to_owned()).finish(),
		None => builder.finish(),
	};
	let encoded = serde_json::to_value(&child).unwrap();
	let decoded: Child = serde_json::from_value(encoded.clone()).unwrap();
	// Assert
	let parent_id: &Option<String> = &child.parent_id;
	assert_eq!(parent_id.as_deref(), key);
	assert_eq!(encoded, json!({"id":child_id,"parent_id":key}));
	assert_eq!(decoded.parent_id, child.parent_id);
}

#[fixture]
fn long_migration(temp_dir: TempDir) -> TempDir {
	let directory = temp_dir.path().join("probe");
	std::fs::create_dir(&directory).unwrap();
	let mut source = String::from(
		"// reinhardt-migration-source: 1\nuse reinhardt::db::migrations::prelude::*;\n\
		 pub fn migration() -> Migration { Migration::new(\"0001_initial\", \"probe\")",
	);
	for _ in 0..256 {
		// These statements are parsed, never applied to a database.
		source.push_str(
			".add_operation(Operation::RunSQL { sql: \"SELECT 1\".to_string(), reverse_sql: None })",
		);
	}
	source.push_str(".atomic(true).with_initial(None).state_only(false).database_only(false) }");
	std::fs::write(directory.join("0001_initial.rs"), source).unwrap();
	temp_dir
}

#[rstest]
fn long_migration_loads_on_an_ordinary_thread_stack(long_migration: TempDir) {
	// Arrange
	let root = long_migration.path().to_owned();
	// Act
	let count = std::thread::Builder::new()
		.stack_size(2 * 1024 * 1024)
		.spawn(move || {
			tokio::runtime::Builder::new_current_thread()
				.enable_all()
				.build()
				.unwrap()
				.block_on(async {
					let migrations = FilesystemSource::new(root).all_migrations().await.unwrap();
					assert_eq!(migrations.len(), 1);
					migrations[0].operations.len()
				})
		})
		.unwrap()
		.join()
		.unwrap();
	// Assert
	assert_eq!(count, 256);
}

#[rstest]
fn query_rendering_preserves_parameter_tokens_inside_identifiers() {
	// Arrange
	let query = Query::select()
		.column(Alias::new("amount$1"))
		.from(Alias::new("ledger$2"))
		.and_where(Expr::col(Alias::new("owner$1")).eq("alice"))
		.to_owned();
	// Act
	let statement = query.to_string(PostgresQueryBuilder);
	// Assert
	assert_eq!(
		statement,
		"SELECT \"amount$1\" FROM \"ledger$2\" WHERE \"owner$1\" = 'alice'"
	);
}

#[rstest]
fn framework_schema_describes_arbitrary_json_and_localized_maps() {
	// Arrange / Act
	let schema = serde_json::to_value(Metadata::schema()).unwrap();
	// Assert
	assert_eq!(schema["properties"]["payload"], json!({}));
	assert_eq!(
		schema["properties"]["translations"],
		json!({"type":"object","additionalProperties":{"type":"string"}})
	);
	assert_eq!(schema["required"], json!(["payload", "translations"]));
}

use reinhardt::test::fixtures::postgres_container;
use reinhardt::test::testcontainers::{ContainerAsync, GenericImage};
use sqlx::PgPool;
use std::sync::Arc;

#[rstest]
#[case::empty(vec![])]
#[case::session_key(vec![0x00, 0x01, 0xff])]
#[case::sql_sensitive(vec![0x00, 0x27, 0x5c, 0x80, 0xff])]
#[tokio::test]
async fn inlined_session_bytes_match_native_parameters(
	#[future] postgres_container: (ContainerAsync<GenericImage>, Arc<PgPool>, u16, String),
	#[case] bytes: Vec<u8>,
	#[values(false, true)] nested: bool,
) {
	// Arrange: Reinhardt's fixture owns this test's isolated PostgreSQL container.
	let (_container, pool, _, _) = postgres_container.await;
	let create = Query::create_table()
		.table("session_bytes")
		.col(ColumnDef::new("value").blob())
		.to_string(PostgresQueryBuilder);
	sqlx::query(&create).execute(pool.as_ref()).await.unwrap();
	let value = Expr::value(bytes.clone()).into_simple_expr();
	let expression = if nested {
		SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![value])
	} else {
		value
	};
	let insert = Query::insert()
		.into_table("session_bytes")
		.columns(["value"])
		.from_subquery(Query::select().expr(expression).to_owned())
		.returning_col("value")
		.to_owned();

	// Act: the inlined and prepared forms must preserve the same bytea value.
	let inline = sqlx::query_scalar::<_, Vec<u8>>(&insert.to_string(PostgresQueryBuilder))
		.fetch_one(pool.as_ref())
		.await
		.unwrap();
	let (prepared, _) = insert.build(PostgresQueryBuilder);
	let bound = sqlx::query_scalar::<_, Vec<u8>>(&prepared)
		.bind(&bytes)
		.fetch_one(pool.as_ref())
		.await
		.unwrap();

	// Assert
	assert_eq!(inline, bytes);
	assert_eq!(bound, bytes);
}

#[rstest]
#[case::plain(false)]
#[case::line_comment(true)]
#[tokio::test]
async fn custom_lease_predicates_preserve_target_isolation(
	#[future] postgres_container: (ContainerAsync<GenericImage>, Arc<PgPool>, u16, String),
	#[case] comment: bool,
) {
	// Arrange: an older unleased neighbor must never replace the requested Run.
	let (_container, pool, _, _) = postgres_container.await;
	let target = Uuid::from_u128(2);
	let predicate = if comment {
		"lease_until IS NULL OR lease_until <= CURRENT_TIMESTAMP -- lease deadline"
	} else {
		"lease_until IS NULL OR lease_until <= CURRENT_TIMESTAMP"
	};
	let query = Query::select()
		.column("id")
		.from("runs")
		.and_where(Expr::col("control").ne("PAUSED"))
		.and_where(Expr::cust(predicate))
		.and_where(Expr::col("id").eq(Expr::value(target)))
		.to_string(PostgresQueryBuilder);
	let fixtures = "WITH runs(id, control, lease_until) AS (VALUES \
        ('00000000-0000-0000-0000-000000000001'::uuid, 'RUNNING', NULL::timestamptz), \
        ('00000000-0000-0000-0000-000000000002'::uuid, 'RUNNING', NULL::timestamptz)) ";

	// Act
	let ids = sqlx::query_scalar::<_, Uuid>(&format!("{fixtures}{query}"))
		.fetch_all(pool.as_ref())
		.await
		.unwrap();

	// Assert
	assert_eq!(ids, vec![target]);
}
