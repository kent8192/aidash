use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const SEMVER: &str = "version ~ '^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)([.](0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?([+][0-9A-Za-z-]+([.][0-9A-Za-z-]+)*)?$'";

// Static expressions only. SeaQuery 0.32 cannot express ALTER TABLE ADD/DROP CHECK;
// only that DDL uses SQL. Foreign keys and indexes use SeaQuery builders.
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
		"jsonb_typeof(metadata) = 'object' AND jsonb_typeof(metadata->'name') = 'object' AND metadata->'name' <> '{}'::jsonb AND jsonb_typeof(metadata->'description') = 'object' AND metadata->'description' <> '{}'::jsonb AND jsonb_typeof(metadata->'config') = 'object' AND jsonb_typeof(COALESCE(metadata->'schema', '{}'::jsonb)) = 'object' AND jsonb_typeof(COALESCE(metadata->'capabilities', '[]'::jsonb)) = 'array' AND jsonb_typeof(COALESCE(metadata->'tags', '[]'::jsonb)) = 'array' AND jsonb_typeof(COALESCE(metadata->'languages', '[]'::jsonb)) = 'array' AND jsonb_typeof(COALESCE(metadata->'skills', '[]'::jsonb)) = 'array'",
	),
	(
		"registry",
		"registry_model_config",
		"kind <> 'model' OR (metadata#>>'{config,provider}' = 'openrouter' AND jsonb_typeof(metadata#>'{config,model_id}') = 'string' AND length(btrim(metadata#>>'{config,model_id}')) > 0 AND jsonb_typeof(metadata#>'{config,endpoint}') = 'string' AND length(btrim(metadata#>>'{config,endpoint}')) > 0 AND CASE WHEN jsonb_typeof(metadata#>'{config,context_window}') = 'number' THEN (metadata#>>'{config,context_window}')::numeric >= 2048 AND trunc((metadata#>>'{config,context_window}')::numeric) = (metadata#>>'{config,context_window}')::numeric ELSE false END AND jsonb_typeof(metadata#>'{config,modalities}') = 'array' AND (metadata#>'{config,modalities}') @> '[\"text\"]'::jsonb AND (NOT (metadata->'config' ? 'reasoning_effort') OR metadata#>'{config,reasoning_effort}' = 'null'::jsonb OR metadata#>>'{config,reasoning_effort}' IN ('none','minimal','low','medium','high','xhigh','max')))",
	),
	(
		"registry",
		"registry_agent_config",
		"kind <> 'agent' OR (jsonb_typeof(metadata#>'{config,instructions}') = 'string' AND length(btrim(metadata#>>'{config,instructions}')) > 0 AND CASE WHEN NOT (metadata->'config' ? 'max_steps') THEN true WHEN jsonb_typeof(metadata#>'{config,max_steps}') = 'number' THEN (metadata#>>'{config,max_steps}')::numeric BETWEEN 1 AND 1000 AND trunc((metadata#>>'{config,max_steps}')::numeric) = (metadata#>>'{config,max_steps}')::numeric ELSE false END)",
	),
	(
		"registry",
		"registry_skill_config",
		"kind <> 'skill' OR (jsonb_typeof(metadata#>'{config,instructions}') = 'string' AND length(btrim(metadata#>>'{config,instructions}')) > 0)",
	),
	(
		"workspaces",
		"workspaces_content",
		"length(btrim(title)) > 0 AND length(btrim(goal)) > 0 AND jsonb_typeof(state) = 'object' AND revision >= 0",
	),
	(
		"tasks",
		"tasks_content",
		"length(btrim(title)) > 0 AND length(btrim(description)) > 0 AND jsonb_typeof(requirements) = 'object' AND revision >= 0",
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
		"jsonb_typeof(manifest) = 'object' AND manifest#>>'{entity,id}' = id AND manifest#>>'{entity,version}' = version",
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

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		for (table, name, expression) in CHECKS {
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
		Ok(())
	}

	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
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
