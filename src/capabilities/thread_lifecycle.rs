//! Thread tombstones keep retained-file ownership separate from conversation UI.
use super::{
	cleanup::{self, Choice, Cleanup},
	contracts::Area,
	records, service, sessions,
};
use crate::{Error, Result, authorization::access::Access, store::Store};
use sea_orm::sea_query::{Alias, Expr, LockType, Order, PostgresQueryBuilder, Query};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(Debug, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FileChoice {
	pub area_id: Uuid,
	pub expected_revision: i64,
	pub choice: Choice,
	pub confirmation_id: Option<Uuid>,
}
#[derive(Debug, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteThread {
	pub idempotency_key: Uuid,
	pub files: Vec<FileChoice>,
}
pub(crate) async fn visible(
	tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
	thread: Uuid,
) -> Result<()> {
	let tombstone: Option<Uuid> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("core_records"))
			.and_where(Expr::col(Alias::new("kind")).eq("thread_tombstone"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(thread)
	.fetch_optional(&mut **tx)
	.await?;
	if tombstone.is_some() {
		return Err(Error::NotFound("thread unavailable".into()));
	}
	Ok(())
}
pub(crate) async fn delete(
	store: &Store,
	access: &mut Access,
	workspace: Uuid,
	thread: Uuid,
	input: DeleteThread,
) -> Result<Value> {
	super::sharing::serialize(access).await?;
	let resource = access.workspace(workspace).await?;
	access.require(&resource, "workspace.read").await?;
	access.require(&resource, "thread.delete").await?;
	let digest = crate::registry::digest(&json!(["thread_delete", workspace, thread, input]));
	if let Some(cached) = sessions::cached(access, input.idempotency_key, &digest).await? {
		return Ok(cached);
	}
	visible(&mut access.tx, thread).await?;
	let channel: crate::collaboration::ChannelThread = sqlx::query_as(
		&sessions::select("channel_threads")
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$2")))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.bind(thread)
	.bind(workspace)
	.fetch_optional(&mut **access.tx)
	.await?
	.ok_or_else(|| Error::NotFound("thread unavailable".into()))?;
	access
		.workspace_record(workspace, "message", channel.root_message_id)
		.await?;
	let mut choices = std::collections::BTreeMap::new();
	for choice in input.files {
		if choices.insert(choice.area_id, choice).is_some() {
			return Err(Error::Invalid("DUPLICATE_FILE_CHOICE".into()));
		}
	}
	let mut results = vec![];
	let mut offset = 0_u64;
	loop {
		// Area rows remain in this query after cleanup, so ordered offset pages
		// keep a stable membership while bounding each database fetch.
		let areas: Vec<Area> = sqlx::query_as(
			&sessions::select("core_areas")
				.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$1")))
				.and_where(Expr::col(Alias::new("thread_id")).eq(Expr::cust("$2")))
				// Deleting a shared conversation never grants management of another
				// subject's private files. Their areas remain available in settings.
				.and_where(Expr::col(Alias::new("owner")).eq(Expr::cust("$3")))
				.order_by(Alias::new("id"), Order::Asc)
				.limit(100)
				.offset(offset)
				.to_string(PostgresQueryBuilder),
		)
		.bind(workspace)
		.bind(thread)
		.bind(&access.identity.subject)
		.fetch_all(&mut **access.tx)
		.await?;
		if areas.is_empty() {
			break;
		}
		offset += areas.len() as u64;
		for area in areas {
			let choice = choices
				.remove(&area.id)
				.ok_or_else(|| Error::Conflict("CHOOSE_RETENTION_FOR_EACH_AREA".into()))?;
			let mut area = cleanup::load(access, area.id, "file.manage").await?;
			if area.revision != choice.expected_revision {
				return Err(Error::Conflict("AREA_REVISION_CHANGED".into()));
			}
			if !matches!(
				area.state.as_str(),
				"active" | "retained" | "recoverable" | "deleted"
			) {
				return Err(Error::Conflict(
					"AREA_BUSY: stop and reconcile execution before deleting the thread".into(),
				));
			}
			let keep = matches!(choice.choice, Choice::Keep);
			super::python::release(store, access, &area, "thread_deleted").await?;
			let result = cleanup::prepare(
				store,
				access,
				area.id,
				Cleanup {
					idempotency_key: Uuid::new_v4(),
					expected_revision: area.revision,
					choice: choice.choice,
					confirmation_id: choice.confirmation_id,
				},
			)
			.await?;
			if keep && area.state == "active" {
				area.generation += 1;
				area.epoch += 1;
				area.state = "retained".into();
				service::publish(store, access, &mut area).await?;
				cleanup::persist(access, &area).await?;
				let mut record = records::get(access, result.operation_id, "cleanup").await?;
				record.data["snapshot"] = area.manifest.clone();
				record.data["retained"] = json!(true);
				record.data["generation"] = json!(area.generation);
				record.data["area_revision"] = json!(area.revision);
				records::update(access, &mut record).await?;
			}
			// Cancelling the old generation does not erase its journal or bytes.
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
			results.push(json!(result));
		}
	}
	if !choices.is_empty() {
		return Err(Error::Conflict("CHOOSE_RETENTION_FOR_EACH_AREA".into()));
	}
	records::insert(access,thread,None,"thread_tombstone","deleted",json!({"workspace_id":workspace,"root_message_id":channel.root_message_id,"file_choices":results}),None).await?;
	let result = json!({"thread_id":thread,"state":"deleted","file_operations":results});
	sessions::cache(access, input.idempotency_key, &digest, &result).await?;
	store
		.event(
			&mut access.tx,
			Some(workspace),
			"message.thread_deleted",
			json!({"thread_id":thread}),
		)
		.await?;
	Ok(result)
}
