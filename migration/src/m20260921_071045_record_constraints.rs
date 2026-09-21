use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const SEMVER: &str = "version ~ '^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)([.](0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?([+][0-9A-Za-z-]+([.][0-9A-Za-z-]+)*)?$'";

// Unicode White_Space, matching Rust str::trim without locale-dependent regexes.
const WHITESPACE_SQL: &str = r"U&'\0009\000A\000B\000C\000D\0020\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000'";

// SeaQuery 0.32.7 TableAlterStatement supports column/foreign-key changes, but
// not ADD/DROP CHECK (TableCreateStatement::check only applies to new tables).
// Keep this existing-table DDL exception in SeaORM's transactional migration;
// foreign keys and indexes use SchemaManager's SeaQuery builders.
// https://docs.rs/sea-query/0.32.7/sea_query/table/struct.TableAlterStatement.html
// https://www.sea-ql.org/SeaORM/docs/1.1.x/migration/writing-migration/#using-raw-sql
// Expressions and identifiers below are static, never application input.
const CHECKS: &[(&str, &str, &str)] = &[
	("registry", "registry_semver", SEMVER),
	("packages", "packages_semver", SEMVER),
	(
		"registry",
		"registry_identity",
		"id ~ '^[a-zA-Z0-9][a-zA-Z0-9._-]{0,99}$' AND metadata->'id' = to_jsonb(id) AND metadata->'version' = to_jsonb(version) AND metadata->'kind' = to_jsonb(kind)",
	),
	(
		"registry",
		"registry_metadata_shape",
		// Strict paths preserve array values; silent mode lets shape checks reject missing keys.
		"jsonb_typeof(metadata) = 'object' AND jsonb_typeof(metadata->'name') = 'object' AND metadata->'name' <> '{}'::jsonb AND NOT jsonb_path_exists(metadata, 'strict $.name.* ? (@.type() != \"string\")', '{}'::jsonb, true) AND jsonb_typeof(metadata->'description') = 'object' AND metadata->'description' <> '{}'::jsonb AND NOT jsonb_path_exists(metadata, 'strict $.description.* ? (@.type() != \"string\")', '{}'::jsonb, true) AND jsonb_typeof(metadata->'config') = 'object' AND jsonb_typeof(COALESCE(metadata->'schema', '{}'::jsonb)) = 'object' AND jsonb_typeof(COALESCE(metadata->'capabilities', '[]'::jsonb)) = 'array' AND jsonb_typeof(COALESCE(metadata->'tags', '[]'::jsonb)) = 'array' AND jsonb_typeof(COALESCE(metadata->'languages', '[]'::jsonb)) = 'array' AND jsonb_typeof(COALESCE(metadata->'skills', '[]'::jsonb)) = 'array'",
	),
	(
		"registry",
		"registry_model_config",
		"kind <> 'model' OR (metadata#>>'{config,provider}' = 'openrouter' AND jsonb_typeof(metadata#>'{config,model_id}') = 'string' AND length(btrim(metadata#>>'{config,model_id}', {whitespace})) > 0 AND jsonb_typeof(metadata#>'{config,endpoint}') = 'string' AND length(btrim(metadata#>>'{config,endpoint}', {whitespace})) > 0 AND CASE WHEN jsonb_typeof(metadata#>'{config,context_window}') = 'number' THEN (metadata#>>'{config,context_window}')::numeric >= 2048 AND trunc((metadata#>>'{config,context_window}')::numeric) = (metadata#>>'{config,context_window}')::numeric ELSE false END AND jsonb_typeof(metadata#>'{config,modalities}') = 'array' AND (metadata#>'{config,modalities}') @> '[\"text\"]'::jsonb AND (NOT (metadata->'config' ? 'reasoning_effort') OR metadata#>'{config,reasoning_effort}' = 'null'::jsonb OR metadata#>>'{config,reasoning_effort}' IN ('none','minimal','low','medium','high','xhigh','max')))",
	),
	(
		"registry",
		"registry_agent_config",
		"kind <> 'agent' OR (jsonb_typeof(metadata#>'{config,instructions}') = 'string' AND length(btrim(metadata#>>'{config,instructions}', {whitespace})) > 0 AND CASE WHEN NOT (metadata->'config' ? 'max_steps') THEN true WHEN jsonb_typeof(metadata#>'{config,max_steps}') = 'number' THEN (metadata#>>'{config,max_steps}')::numeric BETWEEN 1 AND 1000 AND trunc((metadata#>>'{config,max_steps}')::numeric) = (metadata#>>'{config,max_steps}')::numeric ELSE false END)",
	),
	(
		"registry",
		"registry_skill_config",
		"kind <> 'skill' OR (jsonb_typeof(metadata#>'{config,instructions}') = 'string' AND length(btrim(metadata#>>'{config,instructions}', {whitespace})) > 0)",
	),
	(
		"workspaces",
		"workspaces_content",
		"length(btrim(title, {whitespace})) > 0 AND length(btrim(goal, {whitespace})) > 0 AND jsonb_typeof(state) = 'object' AND revision >= 0",
	),
	(
		"tasks",
		"tasks_content",
		"length(btrim(title, {whitespace})) > 0 AND length(btrim(description, {whitespace})) > 0 AND jsonb_typeof(requirements) = 'object' AND revision >= 0",
	),
	(
		"tasks",
		"tasks_no_self_reference",
		"(parent_id IS NULL OR parent_id <> id) AND NOT (id = ANY(dependencies)) AND array_position(dependencies, NULL) IS NULL",
	),
	("runs", "runs_counters", "step >= 0 AND revision >= 0"),
	(
		"runs",
		"runs_lease",
		"(lease_owner IS NULL) = (lease_until IS NULL)",
	),
	(
		"installations",
		"installations_config",
		"jsonb_typeof(config) = 'object'",
	),
	(
		"packages",
		"packages_identity",
		"jsonb_typeof(manifest) = 'object' AND manifest#>'{entity,id}' = to_jsonb(id) AND manifest#>'{entity,version}' = to_jsonb(version)",
	),
	(
		"semantic_indexes",
		"semantic_indexes_revision",
		"revision > 0 AND jsonb_typeof(spec) = 'object'",
	),
	(
		"semantic_entries",
		"semantic_entries_counters",
		"revision > 0 AND index_revision > 0 AND attempts >= 0",
	),
	(
		"semantic_run_reads",
		"semantic_run_reads_revision",
		"revision > 0",
	),
	(
		"authorization_revisions",
		"authorization_revisions_positive",
		"revision > 0",
	),
	(
		"generation_policy_history",
		"generation_policy_history_revision",
		"revision > 0",
	),
];

// These expression builders only receive static migration identifiers/paths.
fn object_fields(value: &str, fields: &[&str]) -> String {
	let keys = fields
		.iter()
		.map(|key| format!("'{key}'"))
		.collect::<Vec<_>>()
		.join(",");
	format!(
		"CASE WHEN jsonb_typeof({value}) = 'object' THEN ({value} - ARRAY[{keys}]::text[]) = '{{}}'::jsonb ELSE false END"
	)
}
fn string_array(value: &str) -> String {
	format!(
		"jsonb_typeof({value}) = 'array' AND NOT jsonb_path_exists({value}, 'strict $[*] ? (@.type() != \"string\")', '{{}}'::jsonb, true)"
	)
}
fn optional_string(value: &str) -> String {
	format!("jsonb_typeof(COALESCE({value}, 'null'::jsonb)) IN ('string', 'null')")
}
fn unsigned(value: &str) -> String {
	format!(
		"CASE WHEN jsonb_typeof({value}) = 'number' AND ({value})::text ~ '^(0|[1-9][0-9]*)$' THEN ({value})::text::numeric <= 18446744073709551615 ELSE false END"
	)
}
fn entity_ref(value: &str) -> String {
	// EntityRef allows extra keys, but both identity strings are required.
	format!(
		"jsonb_typeof({value}) = 'object' AND jsonb_typeof({value}->'id') = 'string' AND jsonb_typeof({value}->'version') = 'string'"
	)
}
fn checks() -> Vec<(&'static str, &'static str, String)> {
	let mut checks = Vec::new();
	for &(table, name, expression) in CHECKS {
		let mut parts = vec![expression.replace("{whitespace}", WHITESPACE_SQL)];
		match name {
			"registry_metadata_shape" => {
				parts.push(object_fields(
					"metadata",
					&[
						"id",
						"version",
						"kind",
						"name",
						"description",
						"capabilities",
						"tags",
						"languages",
						"skills",
						"schema",
						"config",
					],
				));
				for field in ["capabilities", "tags", "languages", "skills"] {
					parts.push(string_array(&format!(
						"COALESCE(metadata->'{field}', '[]'::jsonb)"
					)));
				}
			}
			"registry_model_config" => {
				let config = "(metadata->'config')";
				let shape = [
					object_fields(
						config,
						&[
							"provider",
							"model_id",
							"endpoint",
							"credential_env",
							"reasoning_effort",
							"context_window",
							"modalities",
							"cost",
						],
					),
					format!("{config} ? 'cost'"),
					optional_string("metadata#>'{config,credential_env}'"),
					string_array("(metadata#>'{config,modalities}')"),
					unsigned("(metadata#>'{config,context_window}')"),
				];
				// Value is intentionally untyped: cost may contain any JSON value.
				parts.push(format!("kind <> 'model' OR ({})", shape.join(" AND ")));
			}
			"registry_agent_config" => {
				parts.push(format!(
					"kind <> 'agent' OR ({})",
					entity_ref("(metadata#>'{config,model}')")
				));
			}
			"tasks_content" => {
				parts.push(object_fields(
					"requirements",
					&[
						"kind",
						"query",
						"capability",
						"language",
						"skill",
						"tag",
						"model",
					],
				));
				parts.push("NOT jsonb_path_exists(requirements, 'strict $.* ? (@.type() != \"string\" && @.type() != \"null\")', '{}'::jsonb, true)".into());
			}
			"runs_counters" => parts.push(
				"jsonb_typeof(context) = 'object' AND jsonb_typeof(pending) = 'object'".into(),
			),
			"semantic_indexes_revision" => {
				parts.push(object_fields(
					"spec",
					&[
						"embedding",
						"vector",
						"enabled",
						"auto_context",
						"max_sources",
						"max_results",
						"max_result_tokens",
						"max_input_bytes",
					],
				));
				for field in ["enabled", "auto_context"] {
					parts.push(format!("jsonb_typeof(spec->'{field}') = 'boolean'"));
				}
				for field in [
					"max_sources",
					"max_results",
					"max_result_tokens",
					"max_input_bytes",
				] {
					parts.push(unsigned(&format!("(spec->'{field}')")));
				}
				parts.push(object_fields(
					"(spec->'embedding')",
					&[
						"provider",
						"endpoint",
						"credential_env",
						"model",
						"model_version",
						"dimensions",
					],
				));
				parts.push(object_fields(
					"(spec->'vector')",
					&["provider", "endpoint", "credential_env"],
				));
				for section in ["embedding", "vector"] {
					for field in ["provider", "endpoint"] {
						parts.push(format!(
							"jsonb_typeof(spec#>'{{{section},{field}}}') = 'string'"
						));
					}
					parts.push(optional_string(&format!(
						"spec#>'{{{section},credential_env}}'"
					)));
				}
				for field in ["model", "model_version"] {
					parts.push(format!(
						"jsonb_typeof(spec#>'{{embedding,{field}}}') = 'string'"
					));
				}
				parts.push(unsigned("(spec#>'{embedding,dimensions}')"));
			}
			"semantic_entries_counters" => {
				let uuid =
					"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}";
				let memory = object_fields("source", &["kind", "text"]);
				let reference = object_fields("source", &["kind", "id"]);
				parts.push(format!("CASE source->>'kind' WHEN 'memory' THEN ({memory} AND jsonb_typeof(source->'text') = 'string') WHEN 'artifact' THEN ({reference} AND jsonb_typeof(source->'id') = 'string' AND source->>'id' ~ '^({uuid}|[0-9a-fA-F]{{32}}|urn:uuid:{uuid}|\\{{{uuid}\\}})$') WHEN 'message' THEN ({reference} AND jsonb_typeof(source->'id') = 'string' AND source->>'id' ~ '^({uuid}|[0-9a-fA-F]{{32}}|urn:uuid:{uuid}|\\{{{uuid}\\}})$') ELSE false END"));
			}
			_ => {}
		}
		checks.push((
			table,
			name,
			parts
				.into_iter()
				.map(|part| format!("({part})"))
				.collect::<Vec<_>>()
				.join(" AND "),
		));
	}
	checks
}

const LINKS: &[(&str, &str, &str, &str)] = &[
	("tasks", "tasks_parent_workspace", "parent_id", "tasks"),
	("artifacts", "artifacts_task_workspace", "task_id", "tasks"),
	(
		"generation_requests",
		"generation_requests_task_workspace",
		"task_id",
		"tasks",
	),
	(
		"human_requests",
		"human_requests_run_workspace",
		"run_id",
		"runs",
	),
];

async fn create_dependencies(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
	manager
		.create_table(
			Table::create()
				.table(Alias::new("task_dependencies"))
				.col(ColumnDef::new(Alias::new("task_id")).uuid().not_null())
				.col(ColumnDef::new(Alias::new("workspace_id")).uuid().not_null())
				.col(
					ColumnDef::new(Alias::new("dependency_id"))
						.uuid()
						.not_null(),
				)
				.primary_key(
					Index::create()
						.col(Alias::new("task_id"))
						.col(Alias::new("dependency_id")),
				)
				.foreign_key(
					ForeignKey::create()
						.name("tasks_dependencies_source_workspace")
						.from_tbl(Alias::new("task_dependencies"))
						.from_col(Alias::new("task_id"))
						.from_col(Alias::new("workspace_id"))
						.to_tbl(Alias::new("tasks"))
						.to_col(Alias::new("id"))
						.to_col(Alias::new("workspace_id"))
						.on_delete(ForeignKeyAction::Cascade),
				)
				.foreign_key(
					ForeignKey::create()
						.name("tasks_dependencies_target_workspace")
						.from_tbl(Alias::new("task_dependencies"))
						.from_col(Alias::new("dependency_id"))
						.from_col(Alias::new("workspace_id"))
						.to_tbl(Alias::new("tasks"))
						.to_col(Alias::new("id"))
						.to_col(Alias::new("workspace_id")),
				)
				.to_owned(),
		)
		.await?;
	manager
		.create_index(
			Index::create()
				.name("task_dependencies_target")
				.table(Alias::new("task_dependencies"))
				.col(Alias::new("dependency_id"))
				.col(Alias::new("workspace_id"))
				.to_owned(),
		)
		.await?;
	// Backfill through SeaQuery; the FKs validate historical array references.
	let select = Query::select()
		.distinct()
		.column(Alias::new("id"))
		.column(Alias::new("workspace_id"))
		.expr(Expr::cust("unnest(dependencies)"))
		.from(Alias::new("tasks"))
		.to_owned();
	let insert = Query::insert()
		.into_table(Alias::new("task_dependencies"))
		.columns(["task_id", "workspace_id", "dependency_id"].map(Alias::new))
		.select_from(select)
		.map_err(|error| DbErr::Custom(error.to_string()))?
		.to_owned();
	manager
		.get_connection()
		.execute(manager.get_database_backend().build(&insert))
		.await?;
	// SeaQuery cannot express PostgreSQL trigger/function DDL. The derived table
	// keeps the array API intact while real FKs protect concurrent writes/deletes.
	manager
		.get_connection()
		.execute_unprepared(
			r#"
CREATE FUNCTION sync_task_dependencies() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'UPDATE' THEN
        DELETE FROM task_dependencies WHERE task_id = OLD.id;
    END IF;
    INSERT INTO task_dependencies(task_id, workspace_id, dependency_id)
        SELECT DISTINCT NEW.id, NEW.workspace_id, unnest(NEW.dependencies);
    RETURN NEW;
END $$;
CREATE TRIGGER tasks_dependencies_sync AFTER INSERT OR UPDATE OF id, workspace_id, dependencies
    ON tasks FOR EACH ROW EXECUTE FUNCTION sync_task_dependencies();
CREATE FUNCTION guard_task_dependencies() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF pg_trigger_depth() < 2 THEN
        RAISE EXCEPTION 'task_dependencies is maintained by tasks'
            USING ERRCODE = '23514', CONSTRAINT = 'tasks_dependencies_managed';
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER task_dependencies_guard BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE
    ON task_dependencies FOR EACH STATEMENT EXECUTE FUNCTION guard_task_dependencies();
"#,
		)
		.await?;
	Ok(())
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		for (table, name, expression) in checks() {
			// PostgreSQL CHECK alone accepts NULL; missing required JSON keys must fail.
			manager
				.get_connection()
				.execute_unprepared(&format!(
					"ALTER TABLE \"{table}\" ADD CONSTRAINT \"{name}\" CHECK (COALESCE(({expression}), false))"
				))
				.await?;
		}
		for table in ["tasks", "runs"] {
			manager
				.create_index(
					Index::create()
						.name(format!("{table}_id_workspace_unique"))
						.table(Alias::new(table))
						.col(Alias::new("id"))
						.col(Alias::new("workspace_id"))
						.unique()
						.to_owned(),
				)
				.await?;
		}
		for (table, name, column, target) in LINKS {
			manager
				.create_foreign_key(
					ForeignKey::create()
						.name(*name)
						.from_tbl(Alias::new(*table))
						.from_col(Alias::new(*column))
						.from_col(Alias::new("workspace_id"))
						.to_tbl(Alias::new(*target))
						.to_col(Alias::new("id"))
						.to_col(Alias::new("workspace_id"))
						.to_owned(),
				)
				.await?;
		}
		create_dependencies(manager).await?;
		Ok(())
	}

	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		// Matching DDL exception: SeaQuery has no trigger/function drop builders.
		manager
			.get_connection()
			.execute_unprepared(
				"DROP TRIGGER tasks_dependencies_sync ON tasks; DROP FUNCTION sync_task_dependencies();",
			)
			.await?;
		manager
			.drop_table(
				Table::drop()
					.table(Alias::new("task_dependencies"))
					.to_owned(),
			)
			.await?;
		manager
			.get_connection()
			.execute_unprepared("DROP FUNCTION guard_task_dependencies()")
			.await?;
		for (table, name, _, _) in LINKS.iter().rev() {
			manager
				.drop_foreign_key(
					ForeignKey::drop()
						.table(Alias::new(*table))
						.name(*name)
						.to_owned(),
				)
				.await?;
		}
		for table in ["runs", "tasks"] {
			manager
				.drop_index(
					Index::drop()
						.name(format!("{table}_id_workspace_unique"))
						.table(Alias::new(table))
						.to_owned(),
				)
				.await?;
		}
		for (table, name, _) in CHECKS.iter().rev() {
			manager
				.get_connection()
				.execute_unprepared(&format!(
					"ALTER TABLE \"{table}\" DROP CONSTRAINT \"{name}\""
				))
				.await?;
		}
		Ok(())
	}
}
