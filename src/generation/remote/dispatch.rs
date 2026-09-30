//! Durable provider admission and settlement outbox. Only a PREPARING attempt
//! can cross the I/O boundary; transport retries cannot dispatch it again.
use super::{Finalization, Reserved, Usage};
use crate::{Error, Result, federation::Federation, semantic::remote::Failure, store::Store};
use sea_orm::sea_query::{
	Alias, Asterisk, Expr, LockType, OnConflict, PostgresQueryBuilder, Query,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(sqlx::FromRow)]
pub(crate) struct Record {
	pub usage: Value,
	pub digest: String,
	pub peer_node: String,
	pub boundary: Value,
	pub state: String,
	pub finalization: Option<Value>,
	pub peer_finalized: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Input {
	pub usage: Usage,
	pub boundary: Value,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FinalizeInput {
	pub usage: Usage,
	pub result: Finalization,
}

pub(crate) async fn prepare(store: &Store, input: &Input, peer: &str) -> Result<()> {
	input.usage.validate()?;
	if input.usage.dispatcher_node != store.node_id
		|| serde_json::to_vec(&input.boundary)?.len() > 65536
	{
		return Err(Error::Forbidden);
	}
	let mut tx = store.pool.begin().await?;
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("generation_remote_dispatches"))
			.columns(["attempt_id", "usage", "digest", "peer_node", "boundary"].map(Alias::new))
			.values_panic(["$1", "$2", "$3", "$4", "$5"].map(Expr::cust))
			.on_conflict(OnConflict::new().do_nothing().to_owned())
			.to_string(PostgresQueryBuilder),
	)
	.bind(input.usage.attempt_id)
	.bind(json!(input.usage))
	.bind(input.usage.digest()?)
	.bind(peer)
	.bind(&input.boundary)
	.execute(&mut *tx)
	.await?;
	let record: Record = sqlx::query_as(
		&Query::select()
			.column(Asterisk)
			.from(Alias::new("generation_remote_dispatches"))
			.and_where(Expr::cust("attempt_id=$1"))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.bind(input.usage.attempt_id)
	.fetch_one(&mut *tx)
	.await?;
	if record.digest != input.usage.digest()?
		|| record.boundary != input.boundary
		|| record.peer_node != peer
		|| record.usage != json!(input.usage)
	{
		return Err(Error::Conflict("provider admission binding changed".into()));
	}
	if record.state != "PREPARING" {
		return Err(Error::RemoteSemantic(Failure::Pending));
	}
	tx.commit().await?;
	Ok(())
}
pub(crate) async fn bound(store: &Store, input: &Input) -> Result<Record> {
	let record: Record = sqlx::query_as(
		&Query::select()
			.column(Asterisk)
			.from(Alias::new("generation_remote_dispatches"))
			.and_where(Expr::cust("attempt_id=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(input.usage.attempt_id)
	.fetch_optional(&store.pool)
	.await?
	.ok_or(Error::Forbidden)?;
	if record.digest != input.usage.digest()?
		|| record.boundary != input.boundary
		|| record.usage != json!(input.usage)
	{
		return Err(Error::Forbidden);
	}
	Ok(record)
}
pub(crate) async fn admitted(
	store: &Store,
	input: &Input,
	reservations: &[Reserved],
) -> Result<()> {
	let changed = sqlx::query(
		&Query::update()
			.table(Alias::new("generation_remote_dispatches"))
			.value(Alias::new("state"), "DISPATCHED")
			.value(Alias::new("reservations"), Expr::cust("$3"))
			.and_where(Expr::cust(
				"attempt_id=$1 AND digest=$2 AND state='PREPARING'",
			))
			.to_string(PostgresQueryBuilder),
	)
	.bind(input.usage.attempt_id)
	.bind(input.usage.digest()?)
	.bind(json!(reservations))
	.execute(&store.pool)
	.await?;
	if changed.rows_affected() != 1 {
		return Err(Error::RemoteSemantic(Failure::Pending));
	}
	Ok(())
}
/// Persist the terminal decision before issuing any refund to either owner.
pub(crate) async fn finish(f: &Federation, input: &Input, result: Finalization) -> Result<()> {
	let record = bound(&f.store, input).await?;
	let state = match result {
		Finalization::Aborted {} => "ABORTED",
		Finalization::Settled { .. } => "SETTLED",
	};
	let from = if state == "ABORTED" {
		"PREPARING"
	} else {
		"DISPATCHED"
	};
	let value = json!(result);
	let changed = sqlx::query(
		&Query::update()
			.table(Alias::new("generation_remote_dispatches"))
			.value(Alias::new("state"), Expr::cust("$3"))
			.value(Alias::new("finalization"), Expr::cust("$4"))
			.and_where(Expr::cust("attempt_id=$1 AND state=$2"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(input.usage.attempt_id)
	.bind(from)
	.bind(state)
	.bind(&value)
	.execute(&f.store.pool)
	.await?;
	if changed.rows_affected() != 1
		&& (record.state != state || record.finalization.as_ref() != Some(&value))
	{
		return Err(Error::Conflict(
			"provider dispatch cannot be finalized in this state".into(),
		));
	}
	deliver(f, input.usage.attempt_id).await
}
async fn deliver(f: &Federation, attempt: Uuid) -> Result<()> {
	let record: Record = sqlx::query_as(
		&Query::select()
			.column(Asterisk)
			.from(Alias::new("generation_remote_dispatches"))
			.and_where(Expr::cust(
				"attempt_id=$1 AND state IN ('ABORTED','SETTLED')",
			))
			.to_string(PostgresQueryBuilder),
	)
	.bind(attempt)
	.fetch_optional(&f.store.pool)
	.await?
	.ok_or(Error::Forbidden)?;
	let input = FinalizeInput {
		usage: serde_json::from_value(record.usage)?,
		result: serde_json::from_value(record.finalization.ok_or(Error::Forbidden)?)?,
	};
	let local = super::finalize(&f.store, &input.usage, &input.result).await;
	if !record.peer_finalized {
		let _: bool = crate::authorization::peer::authority_request(
			f,
			&record.peer_node,
			"/scoped/usage/finalize",
			&json!(input),
		)
		.await?;
		if !matches!(
			&local,
			Ok(()) | Err(Error::RemoteSemantic(Failure::ProviderContract))
		) {
			return local;
		}
		sqlx::query(
			&Query::update()
				.table(Alias::new("generation_remote_dispatches"))
				.value(Alias::new("peer_finalized"), true)
				.and_where(Expr::cust("attempt_id=$1"))
				.to_string(PostgresQueryBuilder),
		)
		.bind(attempt)
		.execute(&f.store.pool)
		.await?;
	}
	local
}
/// Durable finalizations can be replayed after either node restarts. No provider
/// is contacted by this reconciliation path and no unknown dispatch is released.
pub(crate) async fn reconcile(f: &Federation) -> Result<()> {
	// A stale preparer can never pass admitted() after this atomic abort. Even
	// if a reservation reply was lost, its owner receives the same finalization.
	sqlx::query(
		&Query::update()
			.table(Alias::new("generation_remote_dispatches"))
			.value(Alias::new("state"), "ABORTED")
			.value(
				Alias::new("finalization"),
				Expr::cust("'{\"state\":\"aborted\"}'::jsonb"),
			)
			.and_where(Expr::cust(
				"state='PREPARING' AND created_at < CLOCK_TIMESTAMP()-INTERVAL '120 seconds'",
			))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&f.store.pool)
	.await?;
	let ids: Vec<Uuid> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("attempt_id"))
			.from(Alias::new("generation_remote_dispatches"))
			.and_where(Expr::cust(
				"state IN ('ABORTED','SETTLED') AND NOT peer_finalized",
			))
			.order_by(Alias::new("created_at"), sea_orm::sea_query::Order::Asc)
			.limit(16)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&f.store.pool)
	.await?;
	for id in ids {
		let _ = deliver(f, id).await;
	}
	Ok(())
}
