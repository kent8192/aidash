//! Explicit, fenced file lifecycle. No run completion or idle timeout deletes data.
use super::{
	contracts::*,
	records::{self, Record},
	service, sessions,
};
use crate::{Error, Result, authorization::access::Access, store::Store};
use chrono::{DateTime, Duration, Utc};
use sea_orm::sea_query::{Alias, Expr, LockType, Order, PostgresQueryBuilder, Query};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Choice {
	Keep,
	Recoverable,
	Irreversible,
}
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Cleanup {
	pub idempotency_key: Uuid,
	pub expected_revision: i64,
	pub choice: Choice,
	pub confirmation_id: Option<Uuid>,
}
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Restore {
	pub idempotency_key: Uuid,
	pub expected_revision: i64,
	pub snapshot_id: Uuid,
	pub thread_id: Uuid,
}
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ManagedArea {
	pub area_id: Uuid,
	pub workspace_id: Uuid,
	pub thread_id: Uuid,
	pub agent_id: String,
	pub owner: String,
	pub state: String,
	pub revision: i64,
	pub generation: i64,
	pub files: usize,
	pub bytes: u64,
	pub snapshot_id: Option<Uuid>,
	pub recovery_expires_at: Option<DateTime<Utc>>,
	pub cleanup_operation_id: Option<Uuid>,
}
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ManagementPage {
	pub items: Vec<ManagedArea>,
	pub next_cursor: Option<Uuid>,
}
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CleanupResult {
	pub operation_id: Uuid,
	pub area_id: Uuid,
	pub state: String,
	pub revision: i64,
	pub recovery_expires_at: Option<DateTime<Utc>>,
}
fn result(record: &Record) -> Result<CleanupResult> {
	Ok(CleanupResult {
		operation_id: record.id,
		area_id: record.area_id.ok_or(Error::Forbidden)?,
		state: record.state.clone(),
		revision: record.data["area_revision"]
			.as_i64()
			.ok_or(Error::Forbidden)?,
		recovery_expires_at: record.expires_at,
	})
}
pub(crate) async fn load(access: &mut Access, id: Uuid, action: &str) -> Result<Area> {
	let area: Area = sqlx::query_as(
		&sessions::select("core_areas")
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$2")))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.bind(&access.identity.tenant)
	.bind(id)
	.fetch_optional(&mut **access.tx)
	.await?
	.ok_or_else(|| Error::NotFound("working area unavailable".into()))?;
	let admin = access
		.snapshot
		.bundle
		.subjects
		.get(&access.identity.subject)
		.is_some_and(|s| s.enabled && s.attributes["file_administrator"] == true);
	if area.owner != access.identity.subject && !admin {
		return Err(Error::NotFound("working area unavailable".into()));
	}
	let workspace = access.workspace(area.workspace_id).await?;
	access.context = workspace.attributes.clone();
	access.require(&workspace, "workspace.read").await?;
	access
		.require(
			&access.resource(
				"working_area",
				id,
				json!({"owner":area.owner,"agent_id":area.agent_id,"thread_id":area.thread_id}),
			),
			action,
		)
		.await?;
	Ok(area)
}
pub(crate) async fn inventory(access: &mut Access, cursor: Option<Uuid>) -> Result<ManagementPage> {
	let rows: Vec<Area> = sqlx::query_as(
		&sessions::select("core_areas")
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
			.and_where(Expr::col(Alias::new("id")).gt(Expr::cust("$2")))
			.order_by(Alias::new("id"), Order::Asc)
			.limit(51)
			.to_string(PostgresQueryBuilder),
	)
	.bind(&access.identity.tenant)
	.bind(cursor.unwrap_or(Uuid::nil()))
	.fetch_all(&mut **access.tx)
	.await?;
	let next_cursor = (rows.len() == 51).then(|| rows[49].id);
	let mut items = vec![];
	for row in rows.into_iter().take(50) {
		let area = match load(access, row.id, "file.manage").await {
			Ok(a) => a,
			Err(Error::NotFound(_) | Error::Forbidden) => continue,
			Err(e) => return Err(e),
		};
		let recovery: Option<Record> = sqlx::query_as(
			&sessions::select("core_records")
				.and_where(Expr::col(Alias::new("area_id")).eq(Expr::cust("$1")))
				.and_where(Expr::col(Alias::new("kind")).eq("cleanup"))
				.and_where(Expr::cust("(state = 'recoverable' OR (state = 'kept' AND data->'retained' = 'true'::jsonb)) AND (data->>'generation')::bigint = $2"))
                .order_by(Alias::new("expires_at"), Order::Desc)
				.limit(1)
				.to_string(PostgresQueryBuilder),
		)
		.bind(area.id).bind(area.generation)
        .fetch_optional(&mut **access.tx)
		.await?;
		let files = service::files(&area)?
			.into_iter()
			.filter(|f| !matches!(f.scope, FileScope::References))
			.collect::<Vec<_>>();
		let cleanup_operation_id = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("id"))
				.from(Alias::new("core_records"))
				.and_where(Expr::col(Alias::new("area_id")).eq(Expr::cust("$1")))
				.and_where(Expr::col(Alias::new("kind")).eq("cleanup"))
				.and_where(Expr::col(Alias::new("state")).is_in(["deleting", "cleanup_failed"]))
				.and_where(Expr::cust("(data->>'generation')::bigint = $2"))
				.order_by(Alias::new("id"), Order::Asc)
				.limit(1)
				.to_string(PostgresQueryBuilder),
		)
		.bind(area.id)
		.bind(area.generation)
		.fetch_optional(&mut **access.tx)
		.await?;
		items.push(ManagedArea {
			cleanup_operation_id,
			area_id: area.id,
			workspace_id: area.workspace_id,
			thread_id: area.thread_id,
			agent_id: area.agent_id,
			owner: area.owner,
			state: area.state,
			revision: area.revision,
			generation: area.generation,
			files: files.len(),
			bytes: files.iter().map(|f| f.size).sum(),
			snapshot_id: recovery.as_ref().map(|r| r.id),
			recovery_expires_at: recovery.and_then(|r| r.expires_at),
		});
	}
	Ok(ManagementPage { items, next_cursor })
}
pub(crate) async fn confirmation(access: &mut Access, id: Uuid, revision: i64) -> Result<Value> {
	let area = load(access, id, "file.delete").await?;
	if area.revision != revision {
		return Err(Error::Conflict("AREA_REVISION_CHANGED".into()));
	}
	let id = Uuid::new_v4();
	let expiry = Utc::now() + Duration::minutes(10);
	records::insert(
		access,
		id,
		Some(area.id),
		"deletion_confirmation",
		"pending",
		json!({"revision":revision,"generation":area.generation}),
		Some(expiry),
	)
	.await?;
	Ok(
		json!({"confirmation_id":id,"area_id":area.id,"revision":revision,"expires_at":expiry,"irreversible":true}),
	)
}
pub(crate) async fn prepare(
	store: &Store,
	access: &mut Access,
	id: Uuid,
	input: Cleanup,
) -> Result<CleanupResult> {
	super::sharing::serialize(access).await?;
	let mut area = load(access, id, "file.manage").await?;
	let digest = crate::registry::digest(&json!(["cleanup", id, input]));
	if let Some(cached) = sessions::cached(access, input.idempotency_key, &digest).await? {
		return result(
			&records::get(
				access,
				serde_json::from_value(cached["operation_id"].clone())?,
				"cleanup",
			)
			.await?,
		);
	}
	if area.revision != input.expected_revision {
		return Err(Error::Conflict("AREA_REVISION_CHANGED".into()));
	}
	if matches!(input.choice, Choice::Irreversible) {
		access
			.require(
				&access.resource("working_area", area.id, json!({"owner":area.owner})),
				"file.delete",
			)
			.await?;
		let mut confirmation = records::get(
			access,
			input
				.confirmation_id
				.ok_or_else(|| Error::Invalid("SEPARATE_DELETION_CONFIRMATION_REQUIRED".into()))?,
			"deletion_confirmation",
		)
		.await?;
		if confirmation.area_id != Some(area.id)
			|| confirmation.owner != access.identity.subject
			|| confirmation.state != "pending"
			|| confirmation.data["revision"] != area.revision
			|| confirmation.data["generation"] != area.generation
			|| confirmation.expires_at.is_none_or(|t| t <= Utc::now())
		{
			return Err(Error::Conflict("DELETION_CONFIRMATION_EXPIRED".into()));
		}
		confirmation.state = "used".into();
		records::update(access, &mut confirmation).await?;
	} else if input.confirmation_id.is_some() {
		return Err(Error::Invalid("UNEXPECTED_CONFIRMATION".into()));
	}
	if matches!(input.choice, Choice::Keep) && area.state == "recoverable" {
		let mut recovery: Record = sqlx::query_as(
			&sessions::select("core_records")
				.and_where(Expr::col(Alias::new("area_id")).eq(Expr::cust("$1")))
				.and_where(Expr::col(Alias::new("kind")).eq("cleanup"))
				.and_where(Expr::col(Alias::new("state")).eq("recoverable"))
				.and_where(Expr::cust("(data->>'generation')::bigint = $2"))
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.bind(area.id)
		.bind(area.generation)
		.fetch_optional(&mut **access.tx)
		.await?
		.ok_or_else(|| Error::Conflict("RECOVERY_UNAVAILABLE".into()))?;
		if recovery.expires_at.is_none_or(|t| t <= Utc::now()) {
			return Err(Error::Conflict("RECOVERY_UNAVAILABLE".into()));
		}
		// Retain the already verified snapshot itself. The expiry worker locks
		// this same area and record, so it cannot erase the bytes after Keep.
		area.manifest = recovery.data["snapshot"].clone();
		area.state = "retained".into();
		area.generation += 1;
		area.epoch += 1;
		service::publish(store, access, &mut area).await?;
		persist(access, &area).await?;
		recovery.state = "kept".into();
		recovery.expires_at = None;
		recovery.data["retained"] = json!(true);
		recovery.data["area_revision"] = json!(area.revision);
		recovery.data["generation"] = json!(area.generation);
		records::update(access, &mut recovery).await?;
		sessions::cache(
			access,
			input.idempotency_key,
			&digest,
			&json!({"operation_id":recovery.id}),
		)
		.await?;
		return result(&recovery);
	}
	if !matches!(input.choice, Choice::Keep) {
		if !matches!(
			area.state.as_str(),
			"active" | "retained" | "recoverable" | "deleted"
		) {
			return Err(Error::Conflict(
				"AREA_BUSY: cancel and reconcile the active writer before cleanup".into(),
			));
		}
		super::python::release(store, access, &area, "cleanup").await?;
	}
	let operation = Uuid::new_v4();
	let mut snapshot = vec![];
	if matches!(input.choice, Choice::Recoverable) {
		if !matches!(area.state.as_str(), "active" | "retained") {
			return Err(Error::Conflict("AREA_UNAVAILABLE".into()));
		}
		sessions::authorize_sources(access, area.workspace_id, &area.constraints).await?;
		for file in service::files(&area)? {
			if matches!(file.scope, FileScope::References) {
				snapshot.push(file);
			} else {
				snapshot.push(
					store
						.capabilities
						.copy_owned(access, area.id, "recovery", &file)
						.await?,
				);
			}
		}
	}
	let kept = matches!(input.choice, Choice::Keep);
	let expires = matches!(input.choice, Choice::Recoverable)
		.then(|| Utc::now() + Duration::seconds(store.capabilities.0.recovery_seconds as i64));
	if !kept {
		area.epoch += 1;
		area.generation += 1;
		area.state = "cleaning".into();
		area.manifest = json!(
			service::files(&area)?
				.into_iter()
				.filter(|f| matches!(f.scope, FileScope::References))
				.collect::<Vec<_>>()
		);
		service::publish(store, access, &mut area).await?;
		persist(access, &area).await?;
		sqlx::query(
			&Query::update()
				.table(Alias::new("runs"))
				.value(Alias::new("control"), "CANCELLED")
				.and_where(
					Expr::col(Alias::new("id")).in_subquery(
						Query::select()
							.column(Alias::new("run_id"))
							.from(Alias::new("core_runs"))
							.and_where(Expr::col(Alias::new("area_id")).eq(Expr::cust("$1")))
							.to_owned(),
					),
				)
				.and_where(Expr::col(Alias::new("phase")).is_not_in([
					"COMPLETED",
					"FAILED",
					"CANCELLED",
				]))
				.to_string(PostgresQueryBuilder),
		)
		.bind(area.id)
		.execute(&mut **access.tx)
		.await?;
		sqlx::query(
			&Query::update()
				.table(Alias::new("core_records"))
				.value(Alias::new("state"), "revoked")
				.and_where(Expr::col(Alias::new("area_id")).eq(Expr::cust("$1")))
				.and_where(Expr::col(Alias::new("kind")).is_in(["grant", "outbound"]))
				.and_where(Expr::col(Alias::new("state")).is_in([
					"active",
					"approved",
					"pending",
					"executing",
				]))
				.to_string(PostgresQueryBuilder),
		)
		.bind(area.id)
		.execute(&mut **access.tx)
		.await?;
	}
	let record=records::insert(access,operation,Some(area.id),"cleanup",if kept{"kept"}else{"deleting"},json!({"choice":input.choice,"snapshot":snapshot,"constraints":area.constraints,"area_revision":area.revision,"generation":area.generation,"credential_id":access.identity.credential_id}),expires).await?;
	sessions::cache(
		access,
		input.idempotency_key,
		&digest,
		&json!({"operation_id":operation}),
	)
	.await?;
	result(&record)
}
pub(crate) async fn persist(access: &mut Access, area: &Area) -> Result<()> {
	sqlx::query(
		&Query::update()
			.table(Alias::new("core_areas"))
			.values([
				(Alias::new("state"), Expr::cust("$2")),
				(Alias::new("generation"), Expr::cust("$3")),
				(Alias::new("epoch"), Expr::cust("$4")),
				(Alias::new("thread_id"), Expr::cust("$5")),
			])
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(area.id)
	.bind(&area.state)
	.bind(area.generation)
	.bind(area.epoch)
	.bind(area.thread_id)
	.execute(&mut **access.tx)
	.await?;
	Ok(())
}

pub(crate) async fn status(access: &mut Access, id: Uuid) -> Result<CleanupResult> {
	let record = records::get(access, id, "cleanup").await?;
	load(
		access,
		record.area_id.ok_or(Error::Forbidden)?,
		"file.manage",
	)
	.await?;
	result(&record)
}
pub(crate) async fn restore(
	store: &Store,
	access: &mut Access,
	id: Uuid,
	input: Restore,
) -> Result<Area> {
	super::sharing::serialize(access).await?;
	let mut area = load(access, id, "file.restore").await?;
	let digest = crate::registry::digest(&json!(["restore", id, input]));
	if let Some(cached) = sessions::cached(access, input.idempotency_key, &digest).await? {
		let restored: Area = serde_json::from_value(cached)?;
		sessions::authorize_sources(access, area.workspace_id, &area.constraints).await?;
		if area.state != "active" || area.generation != restored.generation {
			return Err(Error::Conflict("RESTORED_GENERATION_CHANGED".into()));
		}
		return Ok(area);
	}
	if area.revision != input.expected_revision
		|| !matches!(area.state.as_str(), "recoverable" | "retained")
	{
		return Err(Error::Conflict("AREA_REVISION_OR_STATE_CHANGED".into()));
	}
	let mut record = records::get(access, input.snapshot_id, "cleanup").await?;
	let retained =
		record.state == "kept" && record.data["retained"] == true && area.state == "retained";
	if record.area_id != Some(area.id)
		|| !(record.state == "recoverable" || retained)
		|| (!retained && record.expires_at.is_none_or(|t| t <= Utc::now()))
	{
		return Err(Error::Conflict("RECOVERY_UNAVAILABLE".into()));
	}
	sessions::authorize_sources(access, area.workspace_id, &record.data["constraints"]).await?;
	super::thread_lifecycle::visible(&mut access.tx, input.thread_id).await?;
	if input.thread_id != area.thread_id {
		let root: Option<Uuid> = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("root_message_id"))
				.from(Alias::new("channel_threads"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$2")))
				.to_string(PostgresQueryBuilder),
		)
		.bind(input.thread_id)
		.bind(area.workspace_id)
		.fetch_optional(&mut **access.tx)
		.await?;
		access
			.workspace_record(
				area.workspace_id,
				"message",
				root.ok_or_else(|| Error::NotFound("restoration thread unavailable".into()))?,
			)
			.await?;
	}
	let occupied: Option<Uuid> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("core_areas"))
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
			.and_where(Expr::col(Alias::new("home_node")).eq(Expr::cust("$2")))
			.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$3")))
			.and_where(Expr::col(Alias::new("thread_id")).eq(Expr::cust("$4")))
			.and_where(Expr::col(Alias::new("agent_id")).eq(Expr::cust("$5")))
			.and_where(Expr::col(Alias::new("owner")).eq(Expr::cust("$6")))
			.and_where(Expr::col(Alias::new("id")).ne(Expr::cust("$7")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(&area.tenant)
	.bind(&area.home_node)
	.bind(area.workspace_id)
	.bind(input.thread_id)
	.bind(&area.agent_id)
	.bind(&area.owner)
	.bind(area.id)
	.fetch_optional(&mut **access.tx)
	.await?;
	if occupied.is_some() {
		return Err(Error::Conflict("RESTORATION_THREAD_OCCUPIED".into()));
	}
	let snapshot: Vec<FileEntry> = serde_json::from_value(record.data["snapshot"].clone())?;
	let bytes = snapshot
		.iter()
		.filter(|file| !matches!(file.scope, FileScope::References))
		.try_fold(0_u64, |total, file| total.checked_add(file.size));
	if bytes.is_none_or(|bytes| bytes > store.capabilities.0.working_bytes) {
		return Err(Error::Conflict("WORKING_QUOTA".into()));
	}
	let mut files = vec![];
	for file in snapshot {
		if matches!(file.scope, FileScope::References) {
			// References are mounted afresh from the selected immutable Agent
			// version at admission; restoring files cannot reinstate old mounts.
			continue;
		} else if retained {
			store.capabilities.verified(access, &file).await?;
			if matches!(file.scope, FileScope::Working) {
				// The snapshot is consumed by this transaction. Its reattached
				// working bytes must follow normal replacement/reclamation rules.
				sqlx::query(
					&Query::update()
						.table(Alias::new("core_objects"))
						.value(Alias::new("kind"), "working")
						.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
						.and_where(Expr::col(Alias::new("area_id")).eq(Expr::cust("$2")))
						.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$3")))
						.and_where(Expr::col(Alias::new("kind")).eq("recovery"))
						.to_string(PostgresQueryBuilder),
				)
				.bind(file.file_id)
				.bind(area.id)
				.bind(&area.tenant)
				.execute(&mut **access.tx)
				.await?;
			}
			files.push(file);
		} else {
			files.push(
				store
					.capabilities
					.copy_owned(access, area.id, "working", &file)
					.await?,
			);
		}
	}
	area.manifest = json!(files);
	area.constraints = record.data["constraints"].clone();
	area.generation += 1;
	area.epoch += 1;
	area.thread_id = input.thread_id;
	area.state = "active".into();
	service::publish(store, access, &mut area).await?;
	persist(access, &area).await.map_err(|error| match error {
		Error::Database(ref database)
			if database.as_database_error().is_some_and(|error| {
				error.is_unique_violation() && error.constraint() == Some("core_session_identity")
			}) =>
		{
			Error::Conflict("RESTORATION_THREAD_OCCUPIED".into())
		}
		error => error,
	})?;
	record.state = if retained { "reattached" } else { "restored" }.into();
	record.expires_at = (!retained).then(Utc::now);
	if retained {
		record.data["snapshot"] = json!([]);
	}
	records::update(access, &mut record).await?;
	sessions::cache(access, input.idempotency_key, &digest, &json!(area)).await?;
	Ok(area)
}
pub(crate) async fn erase_job(store: &Store, snapshot: Record) -> Result<()> {
	// Deletion was explicitly authorized and fenced before this worker ran.
	// Its committed lifecycle contract survives user/credential revocation;
	// no authority to restore or read content is created here.
	let mut tx = store.pool.begin().await?;
	let mut area: Area = sqlx::query_as(
		&sessions::select("core_areas")
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$2")))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.bind(snapshot.area_id.ok_or(Error::Forbidden)?)
	.bind(&snapshot.tenant)
	.fetch_one(&mut *tx)
	.await?;
	let mut record: Record = sqlx::query_as(
		&sessions::select("core_records")
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.and_where(Expr::col(Alias::new("kind")).eq("cleanup"))
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$2")))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.bind(snapshot.id)
	.bind(&area.tenant)
	.fetch_one(&mut *tx)
	.await?;
	let expiring = matches!(record.state.as_str(), "restored" | "recoverable")
		&& record.expires_at.is_some_and(|t| t <= Utc::now());
	if !expiring && !matches!(record.state.as_str(), "deleting" | "cleanup_failed") {
		return Ok(());
	}
	let snapshot_files: Vec<FileEntry> = serde_json::from_value(record.data["snapshot"].clone())?;
	let snapshot_ids = snapshot_files
		.iter()
		.filter(|f| !matches!(f.scope, FileScope::References))
		.map(|f| f.file_id)
		.collect::<std::collections::BTreeSet<_>>();
	if expiring {
		if record.state == "recoverable"
			&& (area.state != "recoverable" || record.data["generation"] != area.generation)
		{
			return Err(Error::Conflict("CLEANUP_GENERATION_CHANGED".into()));
		}
		for id in snapshot_ids {
			store
				.capabilities
				.erase_committed(&mut tx, &area.tenant, id)
				.await?;
		}
		if record.state == "recoverable" {
			area.state = "deleted".into();
			area.generation += 1;
			area.epoch += 1;
			area.revision += 1;
		}
		record.state = "expired".into();
		record.data["snapshot"] = json!([]);
		record.expires_at = None;
	} else {
		if area.generation != record.data["generation"]
			|| !matches!(area.state.as_str(), "cleaning" | "cleanup_failed")
		{
			return Err(Error::Conflict("CLEANUP_GENERATION_CHANGED".into()));
		}
		let ids: Vec<Uuid> = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("id"))
				.from(Alias::new("core_objects"))
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
				.and_where(Expr::col(Alias::new("area_id")).eq(Expr::cust("$2")))
				.to_string(PostgresQueryBuilder),
		)
		.bind(&area.tenant)
		.bind(area.id)
		.fetch_all(&mut *tx)
		.await?;
		for id in ids {
			if !snapshot_ids.contains(&id) {
				store
					.capabilities
					.erase_committed(&mut tx, &area.tenant, id)
					.await?;
			}
		}
		record.state = if record.data["choice"] == "recoverable" {
			"recoverable"
		} else {
			"deleted"
		}
		.into();
		area.state = record.state.clone();
	}
	sqlx::query(
		&Query::update()
			.table(Alias::new("core_areas"))
			.values([
				(Alias::new("state"), Expr::cust("$2")),
				(Alias::new("generation"), Expr::cust("$3")),
				(Alias::new("epoch"), Expr::cust("$4")),
				(Alias::new("revision"), Expr::cust("$5")),
			])
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(area.id)
	.bind(&area.state)
	.bind(area.generation)
	.bind(area.epoch)
	.bind(area.revision)
	.execute(&mut *tx)
	.await?;
	records::update_committed(&mut tx, &mut record).await?;
	store
		.event(
			&mut tx,
			Some(area.workspace_id),
			"capability.cleanup_changed",
			json!({"area_id":area.id,"operation_id":record.id,"status":record.state}),
		)
		.await?;
	tx.commit().await?;
	Ok(())
}
pub(crate) async fn run(
	store: Store,
	mut stopping: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
	let mut cursor = Uuid::nil();
	loop {
		if *stopping.borrow() {
			return Ok(());
		}
		let jobs:Vec<Record>=sqlx::query_as(&sessions::select("core_records").and_where(Expr::col(Alias::new("kind")).eq("cleanup")).and_where(Expr::cust("state IN ('deleting','cleanup_failed') OR (state IN ('restored','recoverable') AND expires_at <= CURRENT_TIMESTAMP)")).and_where(Expr::col(Alias::new("id")).gt(Expr::cust("$1"))).order_by(Alias::new("id"),Order::Asc).limit(8).to_string(PostgresQueryBuilder)).bind(cursor).fetch_all(&store.pool).await?;
		if jobs.is_empty() {
			cursor = Uuid::nil();
		}
		for job in jobs {
			let id = job.id;
			cursor = id;
			let area = job.area_id;
			if let Err(error) = Box::pin(erase_job(&store, job)).await {
				let mut tx = store.pool.begin().await?;
				sqlx::query(
					&Query::update()
						.table(Alias::new("core_records"))
						.value(Alias::new("state"), "cleanup_failed")
						.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
						.and_where(Expr::col(Alias::new("state")).eq("deleting"))
						.to_string(PostgresQueryBuilder),
				)
				.bind(id)
				.execute(&mut *tx)
				.await?;
				sqlx::query(
					&Query::update()
						.table(Alias::new("core_areas"))
						.value(Alias::new("state"), "cleanup_failed")
						.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
						.and_where(Expr::col(Alias::new("state")).eq("cleaning"))
						.to_string(PostgresQueryBuilder),
				)
				.bind(area)
				.execute(&mut *tx)
				.await?;
				tx.commit().await?;
				tracing::warn!(%error,"file lifecycle reconciliation pending");
			}
		}
		tokio::select! {_=stopping.changed()=>{},_=tokio::time::sleep(std::time::Duration::from_secs(1))=>{}}
	}
}
