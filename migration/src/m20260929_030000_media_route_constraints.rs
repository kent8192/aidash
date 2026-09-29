use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const OLD_MODEL_KEYS: &str = "ARRAY['provider','model_id','endpoint','credential_env','reasoning_effort','context_window','max_output_tokens','modalities','cost','request_timeout_secs']::text[]";
const NEW_MODEL_KEYS: &str = "ARRAY['provider','model_id','endpoint','credential_env','reasoning_effort','context_window','max_output_tokens','modalities','cost','request_timeout_secs','media_routes']::text[]";
const OLD_INSTALL_KEYS: &str = "ARRAY['provider','model_id','endpoint','credential_env','reasoning_effort','context_window','modalities','cost','request_timeout_secs']::text[]";
const NEW_INSTALL_KEYS: &str = "ARRAY['provider','model_id','endpoint','credential_env','reasoning_effort','context_window','modalities','cost','request_timeout_secs','media_routes']::text[]";
const MODALITY_GUARD: &str = "AND (NOT NEW.config ? 'modalities'";
const ROUTE_GUARD: &str = "AND (NOT NEW.config ? 'media_routes' OR jsonb_typeof(NEW.config->'media_routes') = 'array')\n        AND (NOT NEW.config ? 'modalities'";

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
	let (_, _, base) = super::m20260921_071045_record_constraints::checks(true)
		.into_iter()
		.find(|(_, name, _)| *name == "registry_model_config")
		.ok_or_else(|| DbErr::Migration("model constraint definition is missing".into()))?;
	let mut model_check = replace_once(
		&base,
		"ARRAY['provider','model_id','endpoint','credential_env','reasoning_effort','context_window','max_output_tokens','modalities','cost']::text[]",
		OLD_MODEL_KEYS,
	)?;
	model_check = format!(
		"({model_check}) AND (kind <> 'model' OR ({}))",
		timeout_check("metadata#>'{config,request_timeout_secs}'")
	);
	if enabled {
		model_check = replace_once(&model_check, OLD_MODEL_KEYS, NEW_MODEL_KEYS)?;
		model_check = format!(
			"({model_check}) AND (kind <> 'model' OR (NOT metadata->'config' ? 'media_routes' OR jsonb_typeof(metadata#>'{{config,media_routes}}') = 'array'))"
		);
	}
	let definition: String = db
		.query_one(
			backend.build(
				&Query::select()
					.expr_as(
						Expr::cust(
							"pg_get_functiondef(to_regprocedure('guard_installation_config()'))",
						),
						Alias::new("definition"),
					)
					.to_owned(),
			),
		)
		.await?
		.ok_or_else(|| DbErr::Migration("installation validator is missing".into()))?
		.try_get("", "definition")?;
	let definition = if enabled {
		let definition = replace_once(&definition, OLD_INSTALL_KEYS, NEW_INSTALL_KEYS)?;
		replace_once(&definition, MODALITY_GUARD, ROUTE_GUARD)?
	} else {
		let definition = replace_once(&definition, NEW_INSTALL_KEYS, OLD_INSTALL_KEYS)?;
		replace_once(&definition, ROUTE_GUARD, MODALITY_GUARD)?
	};
	// SeaQuery does not replace named CHECK constraints or PL/pgSQL functions.
	// All identifiers and expressions here are static; an unexpected prior
	// definition aborts before the schema changes.
	db.execute_unprepared(&format!(
		"ALTER TABLE registry DROP CONSTRAINT registry_model_config; \
		 ALTER TABLE registry ADD CONSTRAINT registry_model_config \
		 CHECK (COALESCE(({model_check}), false))"
	))
	.await?;
	db.execute_unprepared(&definition).await?;
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
			"unexpected media route validator definition".into(),
		));
	}
	Ok(source.replacen(from, to, 1))
}
