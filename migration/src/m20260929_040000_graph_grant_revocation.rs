use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		// SeaQuery cannot express a PL/pgSQL trigger function. Grant revocation is
		// authority metadata and must remain possible during an atomic gate.
		manager
			.get_connection()
			.execute_unprepared(GRANT_GUARD)
			.await?;
		Ok(())
	}

	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.get_connection()
			.execute_unprepared(PREVIOUS_GUARD)
			.await?;
		Ok(())
	}
}

const GRANT_GUARD: &str = r#"
CREATE OR REPLACE FUNCTION atomic_write_guard() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE pending uuid;
BEGIN
    IF current_setting('aidash.transaction_control',true) = 'authority'
       AND TG_TABLE_NAME IN ('authorization_bundles','authorization_revisions',
           'authorization_credentials','authorization_decisions',
           'authorization_peer_mappings','authorization_peer_mapping_history',
           'authorization_graph_operator_grants','peers') THEN
        RETURN NULL;
    END IF;
    SELECT transaction_id INTO pending FROM atomic_gate WHERE singleton FOR SHARE NOWAIT;
    IF pending IS NOT NULL AND current_setting('aidash.atomic_transaction',true) IS DISTINCT FROM pending::text THEN
        RAISE EXCEPTION 'atomic transaction visibility pending' USING ERRCODE='55P03';
    END IF;
    RETURN NULL;
END;
$$;
"#;

const PREVIOUS_GUARD: &str = r#"
CREATE OR REPLACE FUNCTION atomic_write_guard() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE pending uuid;
BEGIN
    IF current_setting('aidash.transaction_control',true) = 'authority'
       AND TG_TABLE_NAME IN ('authorization_bundles','authorization_revisions',
           'authorization_credentials','authorization_decisions',
           'authorization_peer_mappings','authorization_peer_mapping_history','peers') THEN
        RETURN NULL;
    END IF;
    SELECT transaction_id INTO pending FROM atomic_gate WHERE singleton FOR SHARE NOWAIT;
    IF pending IS NOT NULL AND current_setting('aidash.atomic_transaction',true) IS DISTINCT FROM pending::text THEN
        RAISE EXCEPTION 'atomic transaction visibility pending' USING ERRCODE='55P03';
    END IF;
    RETURN NULL;
END;
$$;
"#;
