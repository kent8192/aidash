//! Interpreter identities are acknowledgements, not authority. Old code is never
//! replayed to reconstruct a lost heap; the runner freezes idle writers.
use super::{
	contracts::*,
	operations,
	records::{self},
	service, sessions,
};
use crate::{Error, Result, authorization::access::Access, domain::Run, store::Store};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Python {
	pub idempotency_key: Uuid,
	#[schema(min_length = 1, max_length = 65536)]
	pub code: String,
	pub expected_revision: i64,
	pub expected_session_id: Option<Uuid>,
	#[schema(minimum = 1, maximum = 600, default = 120)]
	pub timeout_seconds: Option<u64>,
}
fn fingerprint(store: &Store, access: &Access, run: &Run, area: &Area) -> String {
	crate::registry::digest(
		&json!({"agent":[run.home_node,run.agent_id,run.agent_version],"subjects":access.subjects,"credential":access.identity.credential_id,"policy_revision":access.snapshot.revision,"area_generation":area.generation,"image":store.capabilities.0.runner.as_ref().map(|p|&p.image)}),
	)
}
pub(crate) async fn release(
	store: &Store,
	access: &mut Access,
	area: &Area,
	reason: &str,
) -> Result<()> {
	let mut record = match records::get(access, area.id, "python_session").await {
		Ok(record) => record,
		Err(Error::NotFound(_)) => return Ok(()),
		Err(e) => return Err(e),
	};
	if matches!(record.state.as_str(), "initial" | "frozen" | "running") {
		let sid = record.data["session_id"].as_str().ok_or(Error::Forbidden)?;
		let reply = operations::remote(
			store,
			reqwest::Method::POST,
			&format!("/v1/sessions/{sid}/stop"),
			None,
		)
		.await?;
		if reply["termination_confirmed"] != true {
			return Err(Error::Conflict("PYTHON_TERMINATION_UNCONFIRMED".into()));
		}
	}
	record.state = "reset".into();
	record.data["reset_reason"] = json!(reason);
	records::update(access, &mut record).await
}
pub(crate) async fn prepare(
	store: &Store,
	access: &mut Access,
	run: &Run,
	area: &mut Area,
	input: Python,
	key: &str,
) -> Result<Value> {
	if input.code.is_empty() || input.code.len() > store.capabilities.0.limits.command_bytes {
		return Err(Error::Invalid("PYTHON_CODE_LIMIT".into()));
	}
	let request_digest = crate::registry::digest(&json!(["python", run.id, input]));
	if let Some(cached) = sessions::cached(access, input.idempotency_key, &request_digest).await? {
		let operation = operations::get(
			access,
			serde_json::from_value(cached["operation_id"].clone())?,
		)
		.await?;
		return Ok(json!(
			operations::result(store, access, &operation, 0).await?
		));
	}
	service::available(area)?;
	if !store.capabilities.0.admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	if input.expected_revision != area.revision {
		return Err(Error::Conflict("AREA_REVISION_CHANGED".into()));
	}
	if sessions::status(access, area).await?.active_run_id != Some(run.id)
		|| matches!(run.phase.as_str(), "COMPLETED" | "FAILED" | "CANCELLED")
	{
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	sessions::authorize(access, area, "file.write").await?;
	let health = operations::verified_health(store, true).await?;
	let current = fingerprint(store, access, run, area);
	let mut record=match records::get(access,area.id,"python_session").await {
        Ok(record)=>record,
        Err(Error::NotFound(_))=>records::insert(access,area.id,Some(area.id),"python_session","initial",json!({"session_id":Uuid::new_v4(),"fingerprint":current,"last_revision":area.revision,"last_used":Utc::now(),"instance":health["instance"]}),None).await?,
        Err(e)=>return Err(e)
    };
	let mut reset = record.state == "reset";
	let mut reason = record.data["reset_reason"]
		.as_str()
		.unwrap_or("environment_changed")
		.to_owned();
	if record.data["fingerprint"] != current
		|| record.data["last_revision"] != area.revision
		|| record.data["instance"] != health["instance"]
	{
		reset = true;
		reason = "authority_version_mount_or_runtime_changed".into();
	}
	if record.state == "frozen" {
		let sid = record.data["session_id"].as_str().ok_or(Error::Forbidden)?;
		let live = operations::remote(
			store,
			reqwest::Method::GET,
			&format!("/v1/sessions/{sid}"),
			None,
		)
		.await?;
		if live["live"] != true {
			reset = true;
			reason = "interpreter_lost_or_idle".into();
		}
		let last: chrono::DateTime<Utc> = serde_json::from_value(record.data["last_used"].clone())?;
		if Utc::now() - last > Duration::seconds(store.capabilities.0.idle_seconds as i64) {
			reset = true;
			reason = "idle_timeout".into();
		}
	}
	if reset {
		release(store, access, area, &reason).await?;
		record = records::get(access, area.id, "python_session").await?;
		record.state = "needs_ack".into();
		record.data = json!({"session_id":Uuid::new_v4(),"fingerprint":current,"last_revision":area.revision,"last_used":Utc::now(),"instance":health["instance"],"reset_reason":reason});
		records::update(access, &mut record).await?;
		store
			.event(
				&mut access.tx,
				Some(area.workspace_id),
				"capability.python_reset",
				json!({"area_id":area.id,"session_id":record.data["session_id"],"reason":reason}),
			)
			.await?;
	}
	let sid: Uuid = serde_json::from_value(record.data["session_id"].clone())?;
	if input
		.expected_session_id
		.is_some_and(|expected| expected != sid)
		|| record.state != "initial" && input.expected_session_id != Some(sid)
	{
		return Ok(
			json!({"operation_id":key,"status":"blocked","session_id":sid,"session_reset":true,"reset_reason":record.data["reset_reason"],"packages":super::packages::environment(store,access,area).await?,"error":{"code":"SESSION_RESET","message":"Acknowledge the new session identity before executing code; earlier Python was not replayed.","retryable":false}}),
		);
	}
	let result = operations::prepare_kind(
		store,
		access,
		run,
		area,
		Shell {
			idempotency_key: input.idempotency_key,
			command: input.code,
			timeout_seconds: input.timeout_seconds,
			expected_revision: input.expected_revision,
		},
		"code_interpreter",
		json!({"session_id":sid}),
	)
	.await?;
	record.state = "running".into();
	record.data["last_used"] = json!(Utc::now());
	record.data["operation_id"] = result["operation_id"].clone();
	records::update(access, &mut record).await?;
	sessions::cache(
		access,
		input.idempotency_key,
		&request_digest,
		&json!({"operation_id":result["operation_id"]}),
	)
	.await?;
	Ok(result)
}
pub(crate) async fn completed(
	access: &mut Access,
	area: &Area,
	operation: &operations::Operation,
	observed: &Value,
) -> Result<()> {
	let mut record = records::get(access, area.id, "python_session").await?;
	if record.data["operation_id"] != json!(operation.id) {
		return Err(Error::Conflict("PYTHON_OPERATION_CHANGED".into()));
	}
	record.state = if observed["writer_frozen"] == true {
		"frozen"
	} else {
		"reset"
	}
	.into();
	record.data["last_revision"] = json!(area.revision);
	record.data["last_used"] = json!(Utc::now());
	records::update(access, &mut record).await
}

/// A frozen heap can retain revoked inputs. Stop it using its committed runtime
/// identity, without impersonating the revoked credential or deleting files.
pub(crate) async fn reap(store: &Store, cursor: &mut Uuid) -> Result<()> {
	use sea_orm::sea_query::{Alias, Expr, LockType, Order, PostgresQueryBuilder, Query};
	let rows: Vec<records::Record> = sqlx::query_as(
		&sessions::select("core_records")
			.and_where(Expr::col(Alias::new("kind")).eq("python_session"))
			.and_where(Expr::col(Alias::new("state")).eq("frozen"))
			.and_where(Expr::col(Alias::new("id")).gt(Expr::cust("$1")))
			.order_by(Alias::new("id"), Order::Asc)
			.limit(32)
			.to_string(PostgresQueryBuilder),
	)
	.bind(*cursor)
	.fetch_all(&store.pool)
	.await?;
	if rows.is_empty() {
		*cursor = Uuid::nil();
	}
	for snapshot in rows {
		// One unavailable runtime must not starve unrelated idle/revoked heaps.
		*cursor = snapshot.id;
		let operation: operations::Operation = sqlx::query_as(
			&sessions::select("core_operations")
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.to_string(PostgresQueryBuilder),
		)
		.bind(serde_json::from_value::<Uuid>(
			snapshot.data["operation_id"].clone(),
		)?)
		.fetch_one(&store.pool)
		.await?;
		let identity = crate::authorization::identity::SubjectIdentity {
			credential_id: operation.credential_id,
			tenant: operation.tenant.clone(),
			subject: operation.principal.clone(),
		};
		let last: chrono::DateTime<Utc> =
			serde_json::from_value(snapshot.data["last_used"].clone())?;
		let idle = Utc::now() - last > Duration::seconds(store.capabilities.0.idle_seconds as i64);
		let valid = match Access::begin(store, &identity).await {
			Ok(mut access) => {
				access.subjects = serde_json::from_value(operation.subjects.clone())?;
				let checked = async {
					let run = access.run_for_interaction(operation.run_id).await?;
					sessions::context_authority(&mut access, &run).await?;
					if access.snapshot.revision != operation.policy_revision {
						return Err(Error::Forbidden);
					}
					Ok(())
				}
				.await;
				access.finish(checked).await
			}
			Err(error) => Err(error),
		};
		let reason = match valid {
			_ if !store.capabilities.0.admission => "admission_disabled",
			Ok(()) if !idle => continue,
			Ok(()) => "idle_timeout",
			Err(
				Error::Forbidden | Error::Unauthorized | Error::NotFound(_) | Error::Conflict(_),
			) => "authority_or_source_withdrawn",
			Err(error) => return Err(error),
		};
		let mut tx = store.pool.begin().await?;
		let _: Uuid = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("id"))
				.from(Alias::new("core_areas"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.bind(operation.area_id)
		.fetch_one(&mut *tx)
		.await?;
		let mut current: records::Record = sqlx::query_as(
			&sessions::select("core_records")
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.and_where(Expr::col(Alias::new("kind")).eq("python_session"))
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.bind(snapshot.id)
		.fetch_one(&mut *tx)
		.await?;
		if current.revision != snapshot.revision || current.state != "frozen" {
			continue;
		}
		let sid = current.data["session_id"]
			.as_str()
			.ok_or(Error::Forbidden)?;
		let stopped = operations::remote(
			store,
			reqwest::Method::POST,
			&format!("/v1/sessions/{sid}/stop"),
			None,
		)
		.await?;
		if stopped["termination_confirmed"] != true {
			return Err(Error::Conflict("PYTHON_TERMINATION_UNCONFIRMED".into()));
		}
		current.state = "reset".into();
		current.data["reset_reason"] = json!(reason);
		records::update_committed(&mut tx, &mut current).await?;
		tx.commit().await?;
	}
	Ok(())
}
