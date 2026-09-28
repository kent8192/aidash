use crate::{Error, Result, domain::Run, store::Store};
use chrono::{DateTime, Utc};
use sea_orm::sea_query::{
	Alias, Expr, LockBehavior, LockType, OnConflict, Order, PostgresQueryBuilder, Query,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

pub(super) fn a(s: &str) -> Alias {
	Alias::new(s)
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
	pub version: u32,
	pub node_id: String,
	pub run_id: Uuid,
	pub activation_id: Uuid,
	pub generation: i64,
}
#[derive(Debug, sqlx::FromRow)]
pub(super) struct Obligation {
	pub id: Uuid,
	pub generation: i64,
	pub run_id: Uuid,
	pub run_revision: i64,
	pub state: String,
	pub publication_epoch: i64,
}
impl Obligation {
	pub fn envelope(&self, node_id: &str) -> Envelope {
		Envelope {
			version: 1,
			node_id: node_id.into(),
			run_id: self.run_id,
			activation_id: self.id,
			generation: self.generation,
		}
	}
}
/// Append an independent obligation in the caller's Run/input transaction. Callers
/// retain their existing Run serialization and recipient/authorization decisions.
/// The compatibility triggers also cover direct/old writers; duplicate requests
/// are safe because each disposition reloads and fences the same Run.
pub async fn request_in(tx: &mut Transaction<'_, Postgres>, run_id: Uuid) -> Result<Uuid> {
	let id = Uuid::new_v4();
	let select = Query::select()
		.expr(Expr::val(id))
		.column(a("id"))
		.column(a("revision"))
		.expr(Expr::val("handoff"))
		.from(a("runs"))
		.and_where(Expr::col(a("id")).eq(run_id))
		.to_owned();
	let query = Query::insert()
		.into_table(a("run_activations"))
		.columns([a("id"), a("run_id"), a("run_revision"), a("reason")])
		.select_from(select)
		.map_err(|e| Error::Invalid(e.to_string()))?
		.to_string(PostgresQueryBuilder);
	if sqlx::query(&query)
		.execute(&mut **tx)
		.await?
		.rows_affected()
		!= 1
	{
		return Err(Error::NotFound("activation Run".into()));
	}
	Ok(id)
}

pub(super) enum Handoff {
	Claimed(Box<Run>, Uuid),
	Recorded,
	Invalid,
}

pub(super) async fn claim(store: &Store, envelope: &Envelope, seconds: i32) -> Result<Handoff> {
	let mut tx = store.pool.begin().await?;
	// All paths lock Run before activation; publishers never lock Runs.
	let run: Option<Run> = sqlx::query_as(
		&Query::select()
			.expr(Expr::cust("*"))
			.from(a("runs"))
			.and_where(Expr::col(a("id")).eq(envelope.run_id))
			.lock_with_behavior(LockType::Update, LockBehavior::Nowait)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut *tx)
	.await
	.map_err(crate::transactions::gate::lock_error)?;
	let row: Option<Obligation> = sqlx::query_as(
		&Query::select()
			.expr(Expr::cust("*"))
			.from(a("run_activations"))
			.and_where(Expr::col(a("id")).eq(envelope.activation_id))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut *tx)
	.await?;
	let Some(row) =
		row.filter(|r| r.run_id == envelope.run_id && r.generation == envelope.generation)
	else {
		return Ok(Handoff::Invalid);
	};
	if row.state == "settled" || row.state == "claimed" {
		tx.commit().await?;
		return Ok(Handoff::Recorded);
	}
	let Some(run) = run else {
		settle(&mut tx, row.id, "run_removed").await?;
		tx.commit().await?;
		return Ok(Handoff::Recorded);
	};
	if ["COMPLETED", "FAILED", "CANCELLED"].contains(&run.phase.as_str()) {
		settle(&mut tx, row.id, "terminal").await?;
		tx.commit().await?;
		return Ok(Handoff::Recorded);
	}
	// A later committed Run transition carries its own responsibility. Never
	// infer input observation from this scheduling supersession.
	if run.revision > row.run_revision {
		let newer: bool = sqlx::query_scalar(
			&Query::select()
				.expr(Expr::exists(
					Query::select()
						.expr(Expr::val(1))
						.from(a("run_activations"))
						.and_where(Expr::col(a("run_id")).eq(run.id))
						.and_where(Expr::col(a("generation")).gt(row.generation))
						.and_where(Expr::col(a("run_revision")).gte(run.revision))
						.to_owned(),
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&mut *tx)
		.await?;
		if newer {
			settle(&mut tx, row.id, "newer_transition").await?;
			tx.commit().await?;
			return Ok(Handoff::Recorded);
		}
	}
	let token = Uuid::new_v4();
	if let Some(run) = Store::lease_run_in(&mut tx, token, seconds, Some(run.id)).await? {
		sqlx::query(
			&Query::update()
				.table(a("run_activations"))
				.value(a("state"), "claimed")
				.value(a("lease_token"), token)
				.value(a("run_revision"), run.revision)
				.value(a("claimed_at"), Expr::cust("CURRENT_TIMESTAMP"))
				.value(
					a("due_at"),
					Expr::cust_with_values(
						"CURRENT_TIMESTAMP + make_interval(secs => $1)",
						[seconds],
					),
				)
				.value(a("claim_source"), "notification")
				.value(a("worker_pid"), i64::from(std::process::id()))
				.value(a("disposition"), "lease_committed")
				.and_where(Expr::col(a("id")).eq(row.id))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?;
		tx.commit().await?;
		metrics::counter!("aidash_activation_claims_total", "source" => "notification")
			.increment(1);
		tracing::info!(run_id=%run.id, activation_id=%row.id, generation=row.generation, revision=run.revision, worker_pid=std::process::id(), "activation lease committed");
		return Ok(Handoff::Claimed(Box::new(run), token));
	}
	// Re-evaluate explicit blockers using DB time. NULL means a durable unblock
	// trigger owns the next attempt (pause, area/order, human response).
	let due: Option<DateTime<Utc>> = sqlx::query_scalar(&Query::select().expr(Expr::cust(
        "CASE WHEN control = 'PAUSED' THEN NULL ELSE GREATEST(\
         CASE WHEN lease_until > CURRENT_TIMESTAMP THEN lease_until END, \
         CASE WHEN (pending->>'retry_at')::timestamptz > CURRENT_TIMESTAMP THEN (pending->>'retry_at')::timestamptz END, \
         CASE WHEN phase = 'WAITING' THEN LEAST(\
           CASE WHEN (pending->>'wake_at')::timestamptz > CURRENT_TIMESTAMP THEN (pending->>'wake_at')::timestamptz END,\
           (SELECT expires_at FROM core_records WHERE id::text = runs.pending->>'core_approval_id' AND state = 'pending' AND expires_at > CURRENT_TIMESTAMP)) END) END"
    )).from(a("runs")).and_where(Expr::col(a("id")).eq(run.id)).to_string(PostgresQueryBuilder))
        .fetch_one(&mut *tx).await?;
	sqlx::query(
		&Query::update()
			.table(a("run_activations"))
			.value(a("state"), "deferred")
			.value(a("due_at"), due)
			.value(a("disposition"), "owner_deadline_or_unblock")
			.value(
				a("publication_epoch"),
				Expr::col(a("publication_epoch")).add(1),
			)
			.value(a("published_at"), Expr::cust("NULL"))
			.value(a("publish_until"), Expr::cust("NULL"))
			.value(a("publish_token"), Expr::cust("NULL"))
			.and_where(Expr::col(a("id")).eq(row.id))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	tx.commit().await?;
	metrics::counter!("aidash_activation_deferred_total").increment(1);
	Ok(Handoff::Recorded)
}
async fn settle(tx: &mut Transaction<'_, Postgres>, id: Uuid, reason: &str) -> Result<()> {
	sqlx::query(
		&Query::update()
			.table(a("run_activations"))
			.value(a("state"), "settled")
			.value(a("due_at"), Expr::cust("NULL"))
			.value(a("disposition"), reason)
			.and_where(Expr::col(a("id")).eq(id))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **tx)
	.await?;
	Ok(())
}
pub(super) async fn quarantine(
	store: &Store,
	payload: &[u8],
	reason: &'static str,
	sequence: Option<u64>,
) -> Result<()> {
	let digest = format!("{:x}", Sha256::digest(payload));
	let query = Query::insert()
		.into_table(a("activation_quarantine"))
		.columns([a("digest"), a("reason"), a("stream_sequence")])
		.values_panic([
			digest.into(),
			reason.into(),
			sequence.and_then(|n| i64::try_from(n).ok()).into(),
		])
		.on_conflict(OnConflict::column(a("digest")).do_nothing().to_owned())
		.to_string(PostgresQueryBuilder);
	sqlx::query(&query).execute(&store.pool).await?;
	metrics::counter!("aidash_activation_quarantined_total", "reason" => reason).increment(1);
	Ok(())
}

pub(super) async fn reconcile(store: &Store) -> Result<u64> {
	// Bounded, oldest-first backfill; existing records (including settled ones)
	// avoid repeatedly rediscovering a permanently blocked Run.
	let select = Query::select()
		.column(a("id"))
		.column(a("revision"))
		.expr(Expr::val("reconcile"))
		.from(a("runs"))
		.and_where(Expr::col(a("phase")).is_not_in(["COMPLETED", "FAILED", "CANCELLED"]))
		.and_where(Expr::cust(
			"NOT EXISTS (SELECT 1 FROM run_activations a WHERE a.run_id = runs.id)",
		))
		.order_by(a("updated_at"), Order::Asc)
		.limit(128)
		.to_owned();
	let query = Query::insert()
		.into_table(a("run_activations"))
		.columns([a("run_id"), a("run_revision"), a("reason")])
		.select_from(select)
		.map_err(|e| Error::Invalid(e.to_string()))?
		.to_string(PostgresQueryBuilder);
	Ok(sqlx::query(&query)
		.execute(&store.pool)
		.await?
		.rows_affected())
}

pub(super) async fn publish_batch(store: &Store, token: Uuid) -> Result<Vec<Obligation>> {
	let due = |state: Option<&str>| {
		let mut q = Query::select();
		q.column(a("generation"))
			.from(a("run_activations"))
			.and_where(Expr::col(a("state")).ne("settled"))
			.and_where(Expr::cust("due_at <= CURRENT_TIMESTAMP"))
			.and_where(Expr::cust(
				"publish_until IS NULL OR publish_until <= CURRENT_TIMESTAMP",
			))
			.order_by(a("due_at"), Order::Asc)
			.order_by(a("generation"), Order::Asc)
			.limit(64)
			.lock_with_behavior(LockType::Update, LockBehavior::SkipLocked);
		if let Some(state) = state {
			q.and_where(Expr::col(a("state")).eq(state));
		} else {
			q.and_where(Expr::col(a("state")).is_in(["pending", "deferred"]));
		}
		q.to_owned()
	};
	// ACKed claims remain durable until their owner releases or the lease expires.
	// A heartbeat postpones recovery without issuing a new scheduling generation.
	sqlx::query(&Query::update().table(a("run_activations"))
        .value(a("state"),Expr::cust("CASE WHEN EXISTS(SELECT 1 FROM runs r WHERE r.id = run_activations.run_id AND r.lease_owner = run_activations.lease_token) THEN CASE WHEN EXISTS(SELECT 1 FROM runs r WHERE r.id = run_activations.run_id AND r.lease_until > CURRENT_TIMESTAMP) THEN 'claimed' ELSE 'pending' END ELSE 'settled' END"))
        .value(a("due_at"),Expr::cust("(SELECT GREATEST(CURRENT_TIMESTAMP, lease_until) FROM runs r WHERE r.id = run_activations.run_id AND r.lease_owner = run_activations.lease_token)"))
        .value(a("publication_epoch"),Expr::col(a("publication_epoch")).add(1))
        .value(a("published_at"),Expr::cust("NULL"))
        .and_where(Expr::col(a("generation")).in_subquery(due(Some("claimed"))))
        .to_string(PostgresQueryBuilder)).execute(&store.pool).await?;
	let rows = sqlx::query_as(
		&Query::update()
			.table(a("run_activations"))
			.value(a("state"), "pending")
			.value(
				a("publication_epoch"),
				Expr::cust("publication_epoch + CASE WHEN published_at IS NULL THEN 0 ELSE 1 END"),
			)
			.value(a("published_at"), Expr::cust("NULL"))
			.value(a("publish_token"), token)
			.value(
				a("publish_until"),
				Expr::cust("CURRENT_TIMESTAMP + INTERVAL '5 seconds'"),
			)
			.and_where(Expr::col(a("generation")).in_subquery(due(None)))
			.returning_all()
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&store.pool)
	.await?;
	Ok(rows)
}
pub(super) async fn published(store: &Store, row: &Obligation, token: Uuid) -> Result<()> {
	sqlx::query(
		&Query::update().table(a("run_activations"))
        .value(a("published_at"),Expr::cust("CURRENT_TIMESTAMP"))
        // Independent repair for transport expiry/storage loss. This scans only
        // indexed explicit obligations, never general runnable state.
        .value(a("due_at"),Expr::cust("CURRENT_TIMESTAMP + INTERVAL '5 seconds'"))
        .value(a("publish_until"),Expr::cust("NULL"))
        .and_where(Expr::col(a("id")).eq(row.id))
        .and_where(Expr::col(a("state")).eq("pending"))
        .and_where(Expr::col(a("publication_epoch")).eq(row.publication_epoch))
        .and_where(Expr::col(a("publish_token")).eq(token)).to_string(PostgresQueryBuilder),
	)
	.execute(&store.pool)
	.await?;
	Ok(())
}

pub(super) async fn recover(store: &Store, seconds: i32) -> Result<Option<(Run, Uuid)>> {
	let mut tx = store.pool.begin().await?;
	let token = Uuid::new_v4();
	let Some(run) = Store::lease_run_in(&mut tx, token, seconds, None).await? else {
		tx.commit().await?;
		return Ok(None);
	};
	let id = request_in(&mut tx, run.id).await?;
	sqlx::query(
		&Query::update()
			.table(a("run_activations"))
			.value(a("state"), "claimed")
			.value(a("lease_token"), token)
			.value(a("claimed_at"), Expr::cust("CURRENT_TIMESTAMP"))
			.value(
				a("due_at"),
				Expr::cust_with_values("CURRENT_TIMESTAMP + make_interval(secs => $1)", [seconds]),
			)
			.value(a("claim_source"), "recovery")
			.value(a("worker_pid"), i64::from(std::process::id()))
			.value(a("disposition"), "lease_committed")
			.and_where(Expr::col(a("id")).eq(id))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	tx.commit().await?;
	metrics::counter!("aidash_activation_claims_total", "source" => "recovery").increment(1);
	tracing::info!(run_id=%run.id, activation_id=%id, worker_pid=std::process::id(), "recovery lease committed");
	Ok(Some((run, token)))
}

pub(super) async fn observe(store: &Store) -> Result<()> {
	let (pending, age): (i64, f64) = sqlx::query_as(
		&Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.expr(Expr::cust(
				"COALESCE(EXTRACT(EPOCH FROM CURRENT_TIMESTAMP - MIN(due_at))::double precision,0)",
			))
			.from(a("run_activations"))
			.and_where(Expr::col(a("state")).ne("settled"))
			.and_where(Expr::cust("due_at <= CURRENT_TIMESTAMP"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&store.pool)
	.await?;
	metrics::gauge!("aidash_activation_due_obligations").set(pending as f64);
	metrics::gauge!("aidash_activation_oldest_due_seconds").set(age.max(0.));
	Ok(())
}
