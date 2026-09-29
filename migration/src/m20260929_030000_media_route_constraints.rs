use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const OLD_MODEL_KEYS: &str = "ARRAY['provider','model_id','endpoint','credential_env','reasoning_effort','context_window','max_output_tokens','modalities','cost','request_timeout_secs']::text[]";
const NEW_MODEL_KEYS: &str = "ARRAY['provider','model_id','endpoint','credential_env','reasoning_effort','context_window','max_output_tokens','modalities','cost','request_timeout_secs','media_routes']::text[]";
const OLD_INSTALL_KEYS: &str = "ARRAY['provider','model_id','endpoint','credential_env','reasoning_effort','context_window','modalities','cost','request_timeout_secs']::text[]";
const NEW_INSTALL_KEYS: &str = "ARRAY['provider','model_id','endpoint','credential_env','reasoning_effort','context_window','modalities','cost','request_timeout_secs','media_routes']::text[]";
const MODALITY_GUARD: &str = "AND (NOT NEW.config ? 'modalities'";
const ROUTE_GUARD: &str = "AND (NOT NEW.config ? 'media_routes' OR aidash_media_routes_valid(NEW.config->'media_routes'))\n        AND (NOT NEW.config ? 'modalities'";

// A CHECK constraint cannot inspect array members with a subquery, so this
// immutable validator is shared by the registry CHECK and installation trigger.
const MEDIA_ROUTE_VALIDATOR: &str = r#"
CREATE FUNCTION aidash_media_routes_valid(routes jsonb) RETURNS boolean
LANGUAGE plpgsql IMMUTABLE STRICT AS $$
DECLARE
    route jsonb;
    media_format jsonb;
    verified_at timestamptz;
    expires_at timestamptz;
BEGIN
    IF jsonb_typeof(routes) <> 'array' THEN RETURN false; END IF;
    FOR route IN SELECT value FROM jsonb_array_elements(routes) AS entries(value) LOOP
        IF jsonb_typeof(route) <> 'object' THEN RETURN false; END IF;
        IF NOT route ?& ARRAY['tag','formats','source','verified_at','expires_at']::text[]
           OR (route - ARRAY['tag','formats','source','verified_at','expires_at']::text[]) <> '{}'::jsonb
           OR jsonb_typeof(route->'tag') <> 'string'
           OR jsonb_typeof(route->'formats') <> 'array'
           OR jsonb_typeof(route->'source') <> 'string'
           OR jsonb_typeof(route->'verified_at') <> 'string'
           OR jsonb_typeof(route->'expires_at') <> 'string'
        THEN RETURN false; END IF;
        IF route->>'tag' !~ '^[A-Za-z0-9._/-]+$'
           OR length(btrim(route->>'source')) = 0
           OR jsonb_array_length(route->'formats') = 0
        THEN RETURN false; END IF;
        FOR media_format IN SELECT value FROM jsonb_array_elements(route->'formats') AS formats(value) LOOP
            IF jsonb_typeof(media_format) <> 'string'
               OR media_format #>> '{}' NOT IN ('image/png','image/jpeg','image/gif','image/webp','wav','mp3','m4a','aac','ogg','webm','flac')
            THEN RETURN false; END IF;
        END LOOP;
        IF route->>'verified_at' !~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}([.][0-9]{1,9})?(Z|[+-][0-9]{2}:[0-9]{2})$'
           OR route->>'expires_at' !~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}([.][0-9]{1,9})?(Z|[+-][0-9]{2}:[0-9]{2})$'
        THEN RETURN false; END IF;
        BEGIN
            verified_at := (route->>'verified_at')::timestamptz;
            expires_at := (route->>'expires_at')::timestamptz;
        EXCEPTION WHEN OTHERS THEN
            RETURN false;
        END;
        IF NOT isfinite(verified_at) OR NOT isfinite(expires_at) OR verified_at >= expires_at
        THEN RETURN false; END IF;
    END LOOP;
    RETURN true;
END $$;
"#;

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
	if enabled {
		// SeaQuery cannot define a PL/pgSQL validator used by a CHECK constraint.
		db.execute_unprepared(MEDIA_ROUTE_VALIDATOR).await?;
	}
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
			"({model_check}) AND (kind <> 'model' OR (NOT metadata->'config' ? 'media_routes' OR aidash_media_routes_valid(metadata#>'{{config,media_routes}}')))"
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
	if !enabled {
		db.execute_unprepared("DROP FUNCTION aidash_media_routes_valid(jsonb)")
			.await?;
	}
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
