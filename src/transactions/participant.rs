use super::{LocalStatus, Manifest, coordinator, gate, history, mutation};
use crate::{Error, Result, federation::Federation};
use serde_json::json;
use sqlx::{Acquire, Postgres, Transaction};
use uuid::Uuid;

async fn begin(f: &Federation) -> Result<Transaction<'static, Postgres>> {
	// PostgreSQL rejects changing isolation from a SELECT inside a transaction.
	// Configure the acquired session before BEGIN, and discard it afterward so
	// this transaction's default cannot leak to another control-pool borrower.
	let mut connection = f.store.control_pool.acquire().await?;
	connection.close_on_drop();
	sqlx::query(
		&sea_orm::sea_query::Query::select()
			.expr(
				sea_orm::sea_query::Func::cust(sea_orm::sea_query::Alias::new("set_config")).args(
					[
						sea_orm::sea_query::Expr::val("default_transaction_isolation").into(),
						sea_orm::sea_query::Expr::val("serializable").into(),
						sea_orm::sea_query::Expr::val(false).into(),
					],
				),
			)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.execute(&mut *connection)
	.await?;
	Ok(Transaction::begin(connection, None).await?)
}

async fn load(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> Result<Option<LocalStatus>> {
	Ok(sqlx::query_as(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
			))
			.from(sea_orm::sea_query::Alias::new("atomic_participants"))
			.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
			.lock(sea_orm::sea_query::LockType::Update)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(id)
	.fetch_optional(&mut **tx)
	.await?)
}
fn check(existing: &LocalStatus, manifest: &Manifest) -> Result<()> {
	if existing.coordinator != manifest.coordinator
		|| existing.digest != manifest.digest()?
		|| existing.manifest != json!(manifest)
	{
		return Err(Error::Conflict(
			"transaction ID already has another immutable manifest".into(),
		));
	}
	Ok(())
}
fn sender(f: &Federation, caller: &str, manifest: &Manifest) -> Result<()> {
	manifest.validate()?;
	manifest.local(&f.config.node_id)?;
	if caller != manifest.coordinator {
		return Err(Error::Forbidden);
	}
	Ok(())
}
async fn phase(tx: &mut Transaction<'_, Postgres>, id: Uuid, next: &str) -> Result<LocalStatus> {
	let row = sqlx::query_as(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("atomic_participants"))
			.value(
				sea_orm::sea_query::Alias::new("phase"),
				sea_orm::sea_query::Expr::cust("$2"),
			)
			.value(
				sea_orm::sea_query::Alias::new("updated_at"),
				sea_orm::sea_query::Expr::cust("CURRENT_TIMESTAMP"),
			)
			.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
			.returning_all()
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(id)
	.bind(next)
	.fetch_one(&mut **tx)
	.await?;
	history(
		tx,
		id,
		"participant",
		next,
		"durable participant transition",
	)
	.await?;
	Ok(row)
}
pub async fn reserve(f: &Federation, caller: &str, manifest: &Manifest) -> Result<LocalStatus> {
	sender(f, caller, manifest)?;
	let mut tx = begin(f).await?;
	if let Some(existing) = load(&mut tx, manifest.id).await? {
		check(&existing, manifest)?;
		tx.commit().await?;
		return Ok(existing);
	}
	if caller != f.config.node_id {
		let allowed: bool = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust(
					"EXISTS(SELECT 1 FROM atomic_peer_trust WHERE node_id = $1 AND enabled)",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(caller)
		.fetch_one(&mut *tx)
		.await?;
		if !allowed {
			return Err(Error::Forbidden);
		}
	}
	if gate::exclusive(&mut tx).await?.is_some() {
		return Err(Error::TransactionPending);
	}
	let row = sqlx::query_as(
		&sea_orm::sea_query::Query::insert()
			.into_table(sea_orm::sea_query::Alias::new("atomic_participants"))
			.columns([
				sea_orm::sea_query::Alias::new("id"),
				sea_orm::sea_query::Alias::new("coordinator"),
				sea_orm::sea_query::Alias::new("digest"),
				sea_orm::sea_query::Alias::new("manifest"),
				sea_orm::sea_query::Alias::new("phase"),
			])
			.values_panic([
				sea_orm::sea_query::Expr::cust("$1"),
				sea_orm::sea_query::Expr::cust("$2"),
				sea_orm::sea_query::Expr::cust("$3"),
				sea_orm::sea_query::Expr::cust("$4"),
				sea_orm::sea_query::Expr::cust("'RESERVED'"),
			])
			.returning_all()
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(manifest.id)
	.bind(&manifest.coordinator)
	.bind(manifest.digest()?)
	.bind(json!(manifest))
	.fetch_one(&mut *tx)
	.await?;
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("atomic_gate"))
			.value(
				sea_orm::sea_query::Alias::new("transaction_id"),
				sea_orm::sea_query::Expr::cust("$1"),
			)
			.and_where(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("singleton")),
			))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(manifest.id)
	.execute(&mut *tx)
	.await?;
	history(
		&mut tx,
		manifest.id,
		"participant",
		"RESERVED",
		"node visibility barrier persisted",
	)
	.await?;
	tx.commit().await?;
	Ok(row)
}
pub async fn prepare(f: &Federation, caller: &str, manifest: &Manifest) -> Result<LocalStatus> {
	sender(f, caller, manifest)?;
	let mut tx = begin(f).await?;
	let existing = load(&mut tx, manifest.id)
		.await?
		.ok_or_else(|| Error::Conflict("participant was not reserved".into()))?;
	check(&existing, manifest)?;
	if existing.phase != "RESERVED" {
		tx.commit().await?;
		return Ok(existing);
	}
	if gate::exclusive(&mut tx).await? != Some(manifest.id) {
		return Err(Error::Conflict(
			"participant lost its visibility barrier".into(),
		));
	}
	sqlx::query(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust(
				"SET_CONFIG('aidash.atomic_transaction', $1, TRUE)",
			))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(manifest.id.to_string())
	.execute(&mut *tx)
	.await?;
	let mut validation = tx.begin().await?;
	mutation::apply(&f.store, &mut validation, manifest).await?;
	validation.rollback().await?;
	let row = phase(&mut tx, manifest.id, "PREPARED").await?;
	tx.commit().await?;
	Ok(row)
}
pub async fn finish(f: &Federation, caller: &str, manifest: &Manifest) -> Result<LocalStatus> {
	sender(f, caller, manifest)?;
	// A message is only a wake-up. Its sender cannot supply the decision.
	let proof = coordinator::decision(f, manifest).await?;
	let Some(decision) = proof.decision.as_deref() else {
		return Err(Error::TransactionPending);
	};
	let mut tx = begin(f).await?;
	let existing = load(&mut tx, manifest.id).await?;
	if let Some(existing) = &existing {
		check(existing, manifest)?;
	}
	if decision == "ABORT" {
		if let Some(existing) = existing {
			if matches!(existing.phase.as_str(), "APPLIED" | "COMMITTED") {
				return Err(Error::Conflict("commit cannot be aborted".into()));
			}
			if existing.phase == "ABORTED" {
				tx.commit().await?;
				return Ok(existing);
			}
			if gate::exclusive(&mut tx).await? != Some(manifest.id) {
				return Err(Error::Conflict(
					"participant lost its visibility barrier".into(),
				));
			}
			sqlx::query(
				&sea_orm::sea_query::Query::update()
					.table(sea_orm::sea_query::Alias::new("atomic_gate"))
					.value(
						sea_orm::sea_query::Alias::new("transaction_id"),
						sea_orm::sea_query::Expr::cust("NULL"),
					)
					.and_where(sea_orm::sea_query::SimpleExpr::from(
						sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("singleton")),
					))
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.execute(&mut *tx)
			.await?;
		} else {
			// Abort tombstones prevent a delayed reserve from resurrecting work.
			sqlx::query(
				&sea_orm::sea_query::Query::insert()
					.into_table(sea_orm::sea_query::Alias::new("atomic_participants"))
					.columns([
						sea_orm::sea_query::Alias::new("id"),
						sea_orm::sea_query::Alias::new("coordinator"),
						sea_orm::sea_query::Alias::new("digest"),
						sea_orm::sea_query::Alias::new("manifest"),
						sea_orm::sea_query::Alias::new("phase"),
					])
					.values_panic([
						sea_orm::sea_query::Expr::cust("$1"),
						sea_orm::sea_query::Expr::cust("$2"),
						sea_orm::sea_query::Expr::cust("$3"),
						sea_orm::sea_query::Expr::cust("$4"),
						sea_orm::sea_query::Expr::cust("'ABORTED'"),
					])
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(manifest.id)
			.bind(&manifest.coordinator)
			.bind(manifest.digest()?)
			.bind(json!(manifest))
			.execute(&mut *tx)
			.await?;
		}
		let row = phase(&mut tx, manifest.id, "ABORTED").await?;
		tx.commit().await?;
		return Ok(row);
	}
	let existing =
		existing.ok_or_else(|| Error::Conflict("commit requires a prepared participant".into()))?;
	if existing.phase == "COMMITTED" {
		tx.commit().await?;
		return Ok(existing);
	}
	if !matches!(existing.phase.as_str(), "PREPARED" | "APPLIED") {
		return Err(Error::Conflict(
			"commit requires a prepared participant".into(),
		));
	}
	if gate::exclusive(&mut tx).await? != Some(manifest.id) {
		return Err(Error::Conflict(
			"participant lost its visibility barrier".into(),
		));
	}
	if existing.phase == "PREPARED" {
		sqlx::query(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust(
					"SET_CONFIG('aidash.atomic_transaction', $1, TRUE)",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(manifest.id.to_string())
		.execute(&mut *tx)
		.await?;
		mutation::apply(&f.store, &mut tx, manifest).await?;
		phase(&mut tx, manifest.id, "APPLIED").await?;
	}
	let row = if proof.visible {
		sqlx::query(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("atomic_gate"))
				.value(
					sea_orm::sea_query::Alias::new("transaction_id"),
					sea_orm::sea_query::Expr::cust("NULL"),
				)
				.value(
					sea_orm::sea_query::Alias::new("commit_epoch"),
					sea_orm::sea_query::Expr::cust("commit_epoch + 1"),
				)
				.and_where(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("singleton")),
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?;
		phase(&mut tx, manifest.id, "COMMITTED").await?
	} else {
		load(&mut tx, manifest.id).await?.unwrap()
	};
	tx.commit().await?;
	f.notify.notify_waiters();
	Ok(row)
}

pub async fn recover_once(f: &Federation) -> Result<usize> {
	let pending: Vec<LocalStatus> = sqlx::query_as(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
			))
			.from(sea_orm::sea_query::Alias::new("atomic_participants"))
			.and_where(sea_orm::sea_query::Expr::cust(
				"phase IN ('RESERVED', 'PREPARED', 'APPLIED')",
			))
			.order_by_expr(
				sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
					sea_orm::sea_query::Alias::new("updated_at"),
				)),
				sea_orm::sea_query::Order::Asc,
			)
			.limit(32)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.fetch_all(&f.store.control_pool)
	.await?;
	let mut completed = 0;
	for row in pending {
		let manifest: Manifest = serde_json::from_value(row.manifest)?;
		// No timeout can decide an outcome. Fetching a durable decision is the
		// only way a participant can make progress without a coordinator push.
		match coordinator::decision(f, &manifest).await {
			Ok(proof) if proof.decision.is_some() => {
				match finish(f, &manifest.coordinator, &manifest).await {
					Ok(_) => completed += 1,
					Err(error) => {
						tracing::debug!(id=%manifest.id,%error,"participant recovery waits")
					}
				}
			}
			Ok(_) => {}
			Err(error) => {
				tracing::debug!(id=%manifest.id,%error,"participant decision unavailable")
			}
		}
	}
	Ok(completed)
}
pub async fn run(f: Federation) -> Result<()> {
	loop {
		if let Err(error) = recover_once(&f).await {
			tracing::warn!(%error,"participant recovery failed; barriers retained");
		}
		tokio::time::sleep(std::time::Duration::from_secs(1)).await;
	}
}
