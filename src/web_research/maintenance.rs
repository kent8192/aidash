//! Cache expiry preserves delivered fragments; revocation removes visibility of
//! the entire dependency chain, including journals, SSE and derived outputs.
use crate::{Result, store::Store};
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};

pub(crate) async fn redact(
	tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
	run: uuid::Uuid,
) -> Result<()> {
	sqlx::query(
		&Query::update()
			.table(Alias::new("core_records"))
			.value(Alias::new("state"), "revoked")
			.value(
				Alias::new("data"),
				Expr::cust("jsonb_build_object('run_id',$1::text,'revoked',true)"),
			)
			.value(Alias::new("revision"), Expr::cust("revision+1"))
			.and_where(Expr::cust(
				"kind LIKE 'web.%' AND data->>'run_id'=$1 AND state<>'revoked'",
			))
			.to_string(PostgresQueryBuilder),
	)
	.bind(run.to_string())
	.execute(&mut **tx)
	.await?;
	sqlx::query(
		&Query::update()
			.table(Alias::new("invocations"))
			.value(Alias::new("input"), Expr::cust("'{}'::jsonb"))
			.value(
				Alias::new("result"),
				Expr::cust(
					"'{\"version\":1,\"status\":\"error\",\"error\":{\"code\":\"evidence_revoked\"}}'::jsonb",
				),
			)
			.and_where(Expr::cust(
				"run_id=$1 AND tool IN ('web_search','web_open','web_find')",
			))
			.to_string(PostgresQueryBuilder),
	)
	.bind(run)
	.execute(&mut **tx)
	.await?;
	sqlx::query(&Query::update().table(Alias::new("runs"))
		.value(Alias::new("context"),Expr::cust("jsonb_set(context,'{history}',COALESCE((SELECT jsonb_agg(CASE WHEN e->>'kind'='tool' AND e#>>'{call,name}' IN ('web_search','web_open','web_find') THEN jsonb_build_object('kind','tool','call',jsonb_set(e->'call','{arguments}','{}'::jsonb),'result',jsonb_build_object('status','revoked')) ELSE e END ORDER BY n) FROM jsonb_array_elements(COALESCE(context->'history','[]'::jsonb)) WITH ORDINALITY AS entries(e,n)),'[]'::jsonb)) || '{\"summary\":\"\",\"run_message_summary\":\"\"}'::jsonb"))
		.value(Alias::new("pending"),Expr::cust("'{}'::jsonb"))
		.and_where(Expr::cust("id=$1")).to_string(PostgresQueryBuilder)).bind(run).execute(&mut **tx).await?;
	// Messages and artifacts produced from this Run have a durable dependency
	// ledger. Redact those copies as well as denying their future reads.
	sqlx::query(&Query::update().table(Alias::new("messages"))
		.value(Alias::new("content"),"Web evidence is no longer available.")
		.and_where(Expr::cust("id IN (SELECT resource_id FROM authorization_run_outputs WHERE run_id=$1 AND resource_kind='message')"))
		.to_string(PostgresQueryBuilder)).bind(run).execute(&mut **tx).await?;
	sqlx::query(&Query::update().table(Alias::new("artifacts"))
		.value(Alias::new("content"),Expr::cust("'{\"status\":\"evidence_revoked\"}'::jsonb"))
		.and_where(Expr::cust("id IN (SELECT resource_id FROM authorization_run_outputs WHERE run_id=$1 AND resource_kind='artifact')"))
		.to_string(PostgresQueryBuilder)).bind(run).execute(&mut **tx).await?;
	sqlx::query(
		&Query::update()
			.table(Alias::new("web_runs"))
			.value(
				Alias::new("data"),
				Expr::cust("data || '{\"revoked\":true,\"cache_bytes\":0,\"purged\":true}'::jsonb"),
			)
			.and_where(Expr::cust("run_id=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(run)
	.execute(&mut **tx)
	.await?;
	Ok(())
}

pub async fn purge(store: &Store) -> Result<()> {
	let mut tx = store.pool.begin().await?;
	// Serialize cache accounting with writers, without impersonating a subject.
	let runs:Vec<uuid::Uuid>=sqlx::query_scalar(&Query::select().column(Alias::new("run_id"))
		.from(Alias::new("web_runs")).and_where(Expr::cust("run_id IN (SELECT id FROM runs WHERE home_node=$1) AND data->>'purged' IS DISTINCT FROM 'true' AND (data->>'revoked'='true' OR (data->>'retention_until')::timestamptz<=CURRENT_TIMESTAMP OR EXISTS(SELECT 1 FROM core_records AS d WHERE d.kind='thread_tombstone' AND d.id::text=web_runs.data->>'thread_id') OR EXISTS(SELECT 1 FROM core_records AS c WHERE c.kind='web.document' AND c.state='ready' AND c.data->>'run_id'=web_runs.run_id::text AND (c.expires_at<=CURRENT_TIMESTAMP OR EXISTS(SELECT 1 FROM runs WHERE id=web_runs.run_id AND phase IN ('COMPLETED','FAILED','CANCELLED') AND updated_at<=CURRENT_TIMESTAMP-INTERVAL '59 minutes'))))"))
		.order_by(Alias::new("run_id"),sea_orm::sea_query::Order::Asc).limit(1000)
		.lock(sea_orm::sea_query::LockType::Update).to_string(PostgresQueryBuilder))
		.bind(&store.node_id).fetch_all(&mut *tx).await?;
	for run in runs {
		let expired:bool=sqlx::query_scalar(&Query::select().expr(Expr::cust("COALESCE(data->>'revoked'='true',false) OR (data->>'retention_until')::timestamptz<=CURRENT_TIMESTAMP OR EXISTS(SELECT 1 FROM core_records AS d WHERE d.kind='thread_tombstone' AND d.id::text=web_runs.data->>'thread_id')"))
			.from(Alias::new("web_runs")).and_where(Expr::cust("run_id=$1")).to_string(PostgresQueryBuilder))
			.bind(run).fetch_one(&mut *tx).await?;
		if expired {
			redact(&mut tx, run).await?;
			continue;
		}
		sqlx::query(&Query::update().table(Alias::new("core_records"))
			.value(Alias::new("state"),"expired")
			.value(Alias::new("data"),Expr::cust("data - 'lines' || '{\"size\":0}'::jsonb"))
			.value(Alias::new("revision"),Expr::cust("revision+1"))
			.and_where(Expr::cust("kind='web.document' AND data->>'run_id'=$1 AND state='ready' AND (expires_at<=CURRENT_TIMESTAMP OR EXISTS(SELECT 1 FROM runs WHERE id=$2 AND phase IN ('COMPLETED','FAILED','CANCELLED') AND updated_at<=CURRENT_TIMESTAMP - INTERVAL '59 minutes') OR EXISTS(SELECT 1 FROM core_records AS c WHERE c.kind='thread_tombstone' AND c.id::text=(SELECT data->>'thread_id' FROM web_runs WHERE run_id=$2)))"))
			.to_string(PostgresQueryBuilder)).bind(run.to_string()).bind(run).execute(&mut *tx).await?;
		sqlx::query(&Query::update().table(Alias::new("web_runs"))
			.value(Alias::new("data"),Expr::cust("jsonb_set(data,'{cache_bytes}',to_jsonb(COALESCE((SELECT SUM((r.data->>'size')::bigint) FROM core_records AS r WHERE r.kind='web.document' AND r.state='ready' AND r.data->>'run_id'=$2),0)))"))
			.and_where(Expr::cust("run_id=$1")).to_string(PostgresQueryBuilder)).bind(run).bind(run.to_string()).execute(&mut *tx).await?;
	}
	tx.commit().await?;
	Ok(())
}
pub async fn run(store: Store, mut stopping: tokio::sync::watch::Receiver<bool>) -> Result<()> {
	loop {
		if let Err(error) = purge(&store).await {
			tracing::warn!(%error,"web evidence cache cleanup failed");
		}
		tokio::select! {_=tokio::time::sleep(std::time::Duration::from_secs(60))=>{},_=stopping.changed()=>if *stopping.borrow(){break;}}
	}
	Ok(())
}
