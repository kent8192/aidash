use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const MODEL_KEYS: &str = "'provider','model_id','endpoint','credential_env','reasoning_effort','context_window','max_output_tokens','modalities','cost'";
const INSTALL_KEYS: &str = "'provider','model_id','endpoint','credential_env','reasoning_effort','context_window','modalities','cost'";
const MODEL_GUARD: &str = "IF target_kind = 'model' AND NOT COALESCE(";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		apply(manager, true).await
	}

	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		apply(manager, false).await
	}
}

async fn apply(manager: &SchemaManager<'_>, enabled: bool) -> Result<(), DbErr> {
	let db = manager.get_connection();
	let backend = db.get_database_backend();
	let (_, _, mut model_check) = super::m20260921_071045_record_constraints::checks(true)
		.into_iter()
		.find(|(_, name, _)| *name == "registry_model_config")
		.ok_or_else(|| DbErr::Migration("model constraint definition is missing".into()))?;
	if enabled {
		model_check = replace_once(
			&model_check,
			&format!("ARRAY[{MODEL_KEYS}]::text[]"),
			&format!("ARRAY[{MODEL_KEYS},'request_timeout_secs']::text[]"),
		)?;
		model_check = format!(
			"({model_check}) AND (kind <> 'model' OR ({}))",
			timeout_check("metadata#>'{config,request_timeout_secs}'")
		);
	}

	// Preserve all existing kind-specific trigger checks. Only the model key
	// allowlist and its timeout predicate change; unexpected definitions abort.
	let constraint_exists = Query::select()
		.expr_as(
			Expr::cust(
				"EXISTS (SELECT 1 FROM pg_constraint WHERE conrelid = to_regclass('registry') AND conname = 'registry_model_config')",
			),
			Alias::new("exists"),
		)
	.to_owned();
	let constraint_exists: bool = db
		.query_one(backend.build(&constraint_exists))
		.await?
		.ok_or_else(|| DbErr::Migration("model constraint state is unavailable".into()))?
		.try_get("", "exists")?;
	let query = Query::select()
		.expr_as(
			Expr::cust("pg_get_functiondef(to_regprocedure('guard_installation_config()'))"),
			Alias::new("definition"),
		)
		.to_owned();
	let definition: Option<String> = db
		.query_one(backend.build(&query))
		.await?
		.ok_or_else(|| DbErr::Migration("installation validator is missing".into()))?
		.try_get("", "definition")?;
	let Some(definition) = definition else {
		if enabled {
			return Err(DbErr::Migration("installation validator is missing".into()));
		}
		if !constraint_exists {
			// A parent rollback can already have removed this migration's objects.
			return Ok(());
		}
		// Keep the schema downgrade idempotent if the parent trigger was removed
		// before this migration is replayed; the parent down migration drops this
		// baseline constraint next.
		db.execute_unprepared(&format!(
			"ALTER TABLE registry DROP CONSTRAINT registry_model_config; \
				 ALTER TABLE registry ADD CONSTRAINT registry_model_config \
				 CHECK (COALESCE(({model_check}), false))"
		))
		.await?;
		return Ok(());
	};
	let old_keys = format!("ARRAY[{INSTALL_KEYS}]::text[]");
	let new_keys = format!("ARRAY[{INSTALL_KEYS},'request_timeout_secs']::text[]");
	let new_guard = format!(
		"{MODEL_GUARD}\n        ({}) AND",
		timeout_check("NEW.config->'request_timeout_secs'")
	);
	let definition = if enabled {
		let definition = replace_once(&definition, &old_keys, &new_keys)?;
		replace_once(&definition, MODEL_GUARD, &new_guard)?
	} else {
		let definition = replace_once(&definition, &new_keys, &old_keys)?;
		replace_once(&definition, &new_guard, MODEL_GUARD)?
	};

	// SeaQuery cannot replace named CHECK constraints on existing tables or
	// define PL/pgSQL trigger functions. These DDL statements use only static
	// identifiers/expressions and the existing function's catalog definition.
	db.execute_unprepared(&format!(
		"ALTER TABLE registry DROP CONSTRAINT registry_model_config; \
		 ALTER TABLE registry ADD CONSTRAINT registry_model_config \
		 CHECK (COALESCE(({model_check}), false))"
	))
	.await?;
	db.execute_unprepared(&definition).await?;

	// Revalidate stored overrides through the updated trigger without changing
	// their values. On downgrade, unsupported fields fail the transactional
	// migration instead of silently deleting immutable model configuration.
	let revalidate = Query::update()
		.table(Alias::new("installations"))
		.value(Alias::new("config"), Expr::col(Alias::new("config")))
		.to_owned();
	db.execute(backend.build(&revalidate)).await?;
	Ok(())
}

fn timeout_check(value: &str) -> String {
	format!(
		"CASE WHEN ({value}) IS NULL OR ({value}) = 'null'::jsonb THEN true \
		 WHEN jsonb_typeof({value}) = 'number' AND ({value})::text ~ '^[1-9][0-9]*$' \
		 THEN ({value})::text::numeric BETWEEN 1 AND 4294967295 ELSE false END"
	)
}

fn replace_once(source: &str, from: &str, to: &str) -> Result<String, DbErr> {
	if source.matches(from).count() != 1 {
		return Err(DbErr::Migration(
			"unexpected inference timeout validator definition".into(),
		));
	}
	Ok(source.replacen(from, to, 1))
}
