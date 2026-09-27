//! Reconcile only committed object-ownership and expiry contracts. This worker
//! does not impersonate a revoked user or grant a read/restore permission.
use super::{
	contracts::FileEntry,
	operations,
	records::{self, Record},
	sessions,
};
use crate::{Error, Result, store::Store};
use chrono::Utc;
use sea_orm::sea_query::{Alias, Expr, LockType, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use uuid::Uuid;

async fn reclaim(store: &Store, id: Uuid) -> Result<()> {
	let mut tx = store.pool.begin().await?;
	let mut record: Record = sqlx::query_as(
		&sessions::select("core_records")
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.fetch_one(&mut *tx)
	.await?;
	let expired = record.expires_at.is_some_and(|at| at <= Utc::now());
	let mut files = vec![];
	match record.kind.as_str() {
		"transfer_out" => {
			if record.data["objects_released"] == true
				|| !(expired || matches!(record.state.as_str(), "delivered" | "blocked"))
			{
				return Ok(());
			}
			files = serde_json::from_value::<Vec<FileEntry>>(
				record.data["description"]["files"].clone(),
			)?;
			if expired && !matches!(record.state.as_str(), "delivered" | "blocked") {
				record.state = if record.data["commit_attempted"] == true {
					"expired_unconfirmed"
				} else {
					"expired"
				}
				.into();
			}
			record.data["objects_released"] = json!(true);
		}
		"transfer_in" => {
			if record.data["objects_released"] == true || !(expired || record.state == "committed")
			{
				return Ok(());
			}
			for value in record.data["chunks"]
				.as_object()
				.ok_or(Error::Forbidden)?
				.values()
			{
				files.push(serde_json::from_value::<FileEntry>(value.clone())?);
			}
			let reserved = record.data["reserved"].as_i64().ok_or(Error::Forbidden)?;
			if reserved > 0 {
				sqlx::query(
					&Query::update()
						.table(Alias::new("core_quotas"))
						.value(
							Alias::new("used_bytes"),
							Expr::col(Alias::new("used_bytes")).sub(Expr::cust("$2")),
						)
						.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
						.and_where(Expr::col(Alias::new("used_bytes")).gte(Expr::cust("$2")))
						.to_string(PostgresQueryBuilder),
				)
				.bind(&record.tenant)
				.bind(reserved)
				.execute(&mut *tx)
				.await?;
			}
			if record.state != "committed" {
				record.state = "expired".into();
			}
			record.data["reserved"] = json!(0);
			record.data["chunks"] = json!({});
			record.data["objects_released"] = json!(true);
		}
		"reference" => {
			let removing = record.state == "revoked"
				|| (expired
					&& matches!(record.state.as_str(), "uploading" | "extracting" | "failed"));
			if removing
				&& !record.data["operation_id"].is_null()
				&& !record.data["instance"].is_null()
				&& record.data["runner_released"] != true
			{
				let operation: Uuid = serde_json::from_value(record.data["operation_id"].clone())?;
				let observed = operations::remote(
					store,
					reqwest::Method::POST,
					&format!("/v1/operations/{operation}/cancel"),
					None,
				)
				.await?;
				let undispatched_absent =
					observed["status"] == "absent" && record.data["dispatch_pending"] == true;
				if observed["termination_confirmed"] != true && !undispatched_absent {
					return Err(Error::Conflict("EXTRACTOR_STOP_PENDING".into()));
				}
				if undispatched_absent {
					// This committed intent was never submitted to the runner, so a
					// verified absence is terminal and needs no acknowledgement.
					record.data["dispatch_pending"] = json!(false);
				} else {
					let original: FileEntry =
						serde_json::from_value(record.data["original"].clone())?;
					let digest = crate::registry::digest(&json!(["extract/1", id, original]));
					operations::remote(
						store,
						reqwest::Method::POST,
						&format!("/v1/operations/{operation}/ack"),
						Some(json!({"digest":digest})),
					)
					.await?;
				}
				record.data["runner_released"] = json!(true);
			}
			if !removing && !matches!(record.state.as_str(), "ready" | "extracting") {
				return Ok(());
			}
			for chunk in record.data["chunks"].as_array().ok_or(Error::Forbidden)? {
				files.push(serde_json::from_value::<FileEntry>(chunk["file"].clone())?);
			}
			record.data["chunks"] = json!([]);
			if removing {
				for key in ["original", "extraction"] {
					if let Some(file) =
						serde_json::from_value::<Option<FileEntry>>(record.data[key].clone())?
					{
						files.push(file);
					}
					record.data[key] = Value::Null;
				}
				record.state = if record.state == "revoked" {
					"revoked"
				} else {
					"expired"
				}
				.into();
				record.expires_at = None;
				record.data["objects_released"] = json!(true);
			}
		}
		_ => return Err(Error::Forbidden),
	}
	for file in files {
		// A transfer/reference record may delete its own provisional objects
		// only. Final receiver-area objects never match this guard.
		let kind: Option<String> = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("kind"))
				.from(Alias::new("core_objects"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$2")))
				.and_where(Expr::col(Alias::new("area_id")).is_null())
				.to_string(PostgresQueryBuilder),
		)
		.bind(file.file_id)
		.bind(&record.tenant)
		.fetch_optional(&mut *tx)
		.await?;
		if let Some(kind) = kind {
			let expected = match record.kind.as_str() {
				"transfer_out" => kind == "transfer_snapshot",
				"transfer_in" => kind == "transfer_staging",
				"reference" => matches!(
					kind.as_str(),
					"reference_staging" | "reference_original" | "reference_extraction"
				),
				_ => false,
			};
			if !expected {
				return Err(Error::Forbidden);
			}
			store
				.capabilities
				.erase_committed(&mut tx, &record.tenant, file.file_id)
				.await?;
		}
	}
	records::update_committed(&mut tx, &mut record).await?;
	tx.commit().await?;
	Ok(())
}
// A committed superseded_working marker is a publication tombstone: no current
// manifest or independently owned snapshot can acquire this old object again.
async fn working_objects(store: &Store, cursor: &mut Uuid) -> Result<()> {
	let ids: Vec<Uuid> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("core_objects"))
			.and_where(Expr::col(Alias::new("kind")).eq("superseded_working"))
			.and_where(Expr::col(Alias::new("id")).gt(Expr::cust("$1")))
			.order_by(Alias::new("id"), sea_orm::sea_query::Order::Asc)
			.limit(16)
			.to_string(PostgresQueryBuilder),
	)
	.bind(*cursor)
	.fetch_all(&store.pool)
	.await?;
	if ids.is_empty() {
		*cursor = Uuid::nil();
	}
	for id in ids {
		*cursor = id;
		let mut tx = store.pool.begin().await?;
		let tenant: Option<String> = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("tenant"))
				.from(Alias::new("core_objects"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.and_where(Expr::col(Alias::new("kind")).eq("superseded_working"))
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.fetch_optional(&mut *tx)
		.await?;
		if let Some(tenant) = tenant {
			store
				.capabilities
				.erase_committed(&mut tx, &tenant, id)
				.await?;
		}
		tx.commit().await?;
	}
	Ok(())
}

pub(crate) async fn run(
	store: Store,
	mut stopping: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
	let mut python_cursor = Uuid::nil();
	let mut record_cursor = Uuid::nil();
	let mut object_cursor = None;
	let mut working_cursor = Uuid::nil();
	loop {
		if *stopping.borrow() {
			return Ok(());
		}
		if let Err(error) = working_objects(&store, &mut working_cursor).await {
			tracing::warn!(%error,"superseded working bytes reclamation pending");
		}
		if let Err(error) = super::python::reap(&store, &mut python_cursor).await {
			tracing::warn!(%error,"Python memory reclamation pending");
		}
		if let Err(error) = store
			.capabilities
			.reconcile_orphan_batch(&store.pool, &mut object_cursor)
			.await
		{
			tracing::warn!(%error, "object ownership reconciliation pending");
		}
		let ids: Vec<Uuid> = sqlx::query_scalar(&Query::select().column(Alias::new("id")).from(Alias::new("core_records"))
            .and_where(Expr::col(Alias::new("kind")).is_in(["transfer_in", "transfer_out", "reference"]))
            .and_where(Expr::cust("COALESCE(data->>'objects_released','false') <> 'true'"))
            .and_where(Expr::cust("(kind = 'transfer_out' AND (state IN ('delivered','blocked') OR expires_at <= CURRENT_TIMESTAMP)) OR (kind = 'transfer_in' AND (state = 'committed' OR expires_at <= CURRENT_TIMESTAMP)) OR (kind = 'reference' AND (state = 'revoked' OR expires_at <= CURRENT_TIMESTAMP OR (state IN ('ready','extracting') AND jsonb_array_length(data->'chunks') > 0)))"))
            .and_where(Expr::col(Alias::new("id")).gt(Expr::cust("$1")))
            .order_by(Alias::new("id"), sea_orm::sea_query::Order::Asc)
            .limit(8).to_string(PostgresQueryBuilder)).bind(record_cursor).fetch_all(&store.pool).await?;
		if ids.is_empty() {
			record_cursor = Uuid::nil();
		}
		for id in ids {
			record_cursor = id;
			if let Err(error) = Box::pin(reclaim(&store, id)).await {
				tracing::warn!(%id, %error, "retained object reconciliation pending");
			}
		}
		tokio::select! { _ = stopping.changed() => {}, _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {} }
	}
}
