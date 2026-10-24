//! Interpreter loss never replays old code; explicit acknowledgements admit a fresh session.
use crate::{
	Error, Result,
	ports::capabilities::python::{PythonRepository, PythonScope},
};
use aidash_domain::{
	RunMetadata,
	capabilities::{
		operations::{ShellRequest as Shell, available},
		python::{Python, fingerprint},
		sessions::Area,
	},
};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use uuid::Uuid;
pub async fn release(scope: &mut dyn PythonScope, area: &Area, reason: &str) -> Result<()> {
	let mut record = match scope.load(area.id).await {
		Ok(record) => record,
		Err(Error::NotFound(_)) => return Ok(()),
		Err(e) => return Err(e),
	};
	if matches!(record.state.as_str(), "initial" | "frozen" | "running") {
		let sid = record.data["session_id"].as_str().ok_or(Error::Forbidden)?;
		let reply = scope
			.request("POST", &format!("/v1/sessions/{sid}/stop"), None)
			.await?;
		if reply["termination_confirmed"] != true {
			return Err(Error::Conflict("PYTHON_TERMINATION_UNCONFIRMED".into()));
		}
	}
	record.state = "reset".into();
	record.data["reset_reason"] = json!(reason);
	scope.update(&mut record).await
}
pub async fn prepare(
	scope: &mut dyn PythonScope,
	run: &RunMetadata,
	area: &mut Area,
	input: Python,
	key: &str,
) -> Result<Value> {
	if input.code.is_empty() || input.code.len() > scope.limits()?.command_bytes {
		return Err(Error::Invalid("PYTHON_CODE_LIMIT".into()));
	}
	let request_digest = aidash_domain::registry::rules::digest(&json!(["python", run.id, input]));
	if let Some(cached) = scope.cached(input.idempotency_key, &request_digest).await? {
		return scope
			.previous_result(serde_json::from_value(cached["operation_id"].clone())?)
			.await;
	}
	available(&area.state)?;
	if !scope.limits()?.admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	if input.expected_revision != area.revision {
		return Err(Error::Conflict("AREA_REVISION_CHANGED".into()));
	}
	if scope.current_run(area).await? != Some(run.id) || run.phase().is_terminal() {
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	scope.require_write(area).await?;
	let health = scope.health().await?;
	let image = scope.limits()?.image;
	let current = fingerprint(
		run,
		scope.subjects(),
		scope.credential(),
		scope.policy_revision(),
		area.generation,
		image.as_deref(),
	);
	let mut record=match scope.load(area.id).await {
        Ok(record)=>record,
        Err(Error::NotFound(_))=>scope.create(area.id,json!({"session_id":Uuid::new_v4(),"fingerprint":current,"last_revision":area.revision,"last_used":Utc::now(),"instance":health["instance"]})).await?,
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
		let live = scope
			.request("GET", &format!("/v1/sessions/{sid}"), None)
			.await?;
		if live["live"] != true {
			reset = true;
			reason = "interpreter_lost_or_idle".into();
		}
		let last: chrono::DateTime<Utc> = serde_json::from_value(record.data["last_used"].clone())?;
		if Utc::now() - last > Duration::seconds(scope.limits()?.idle_seconds as i64) {
			reset = true;
			reason = "idle_timeout".into();
		}
	}
	if reset {
		release(scope, area, &reason).await?;
		record = scope.load(area.id).await?;
		record.state = "needs_ack".into();
		record.data = json!({"session_id":Uuid::new_v4(),"fingerprint":current,"last_revision":area.revision,"last_used":Utc::now(),"instance":health["instance"],"reset_reason":reason});
		scope.update(&mut record).await?;
		scope
			.event(
				area.workspace_id,
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
			json!({"operation_id":key,"status":"blocked","session_id":sid,"session_reset":true,"reset_reason":record.data["reset_reason"],"packages":scope.environment(area).await?,"error":{"code":"SESSION_RESET","message":"Acknowledge the new session identity before executing code; earlier Python was not replayed.","retryable":false}}),
		);
	}
	let result = scope
		.prepare(
			area,
			Shell {
				idempotency_key: input.idempotency_key,
				command: input.code,
				timeout_seconds: input.timeout_seconds,
				expected_revision: input.expected_revision,
			},
			json!({"session_id":sid}),
		)
		.await?;
	record.state = "running".into();
	record.data["last_used"] = json!(Utc::now());
	record.data["operation_id"] = result["operation_id"].clone();
	scope.update(&mut record).await?;
	scope
		.cache(
			input.idempotency_key,
			&request_digest,
			&json!({"operation_id":result["operation_id"]}),
		)
		.await?;
	Ok(result)
}
pub async fn completed(
	scope: &mut dyn PythonScope,
	area: &Area,
	operation_id: Uuid,
	observed: &Value,
) -> Result<()> {
	let mut record = scope.load(area.id).await?;
	if record.data["operation_id"] != json!(operation_id) {
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
	scope.update(&mut record).await
}
pub async fn reap(repository: &dyn PythonRepository, cursor: &mut Uuid) -> Result<()> {
	let rows = repository.frozen(*cursor).await?;
	if rows.is_empty() {
		*cursor = Uuid::nil();
	}
	for snapshot in rows {
		// One unavailable runtime must not starve unrelated idle/revoked heaps.
		*cursor = snapshot.id;
		let operation = repository
			.operation(serde_json::from_value(
				snapshot.data["operation_id"].clone(),
			)?)
			.await?;
		let last: chrono::DateTime<Utc> =
			serde_json::from_value(snapshot.data["last_used"].clone())?;
		let idle = Utc::now() - last > Duration::seconds(repository.idle_seconds() as i64);
		let valid = match repository.authority(&operation).await {
			Ok(mut access) => {
				access.set_subjects(serde_json::from_value(operation.subjects.clone())?);
				let checked = async {
					let run = access.interaction_run(operation.run_id).await?;
					access.context_authority(&run).await?;
					if access.policy_revision() != operation.policy_revision {
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
			_ if !repository.admission() => "admission_disabled",
			Ok(()) if !idle => continue,
			Ok(()) => "idle_timeout",
			Err(
				Error::Forbidden | Error::Unauthorized | Error::NotFound(_) | Error::Conflict(_),
			) => "authority_or_source_withdrawn",
			Err(error) => return Err(error),
		};
		let mut scope = repository.begin().await?;
		let result = async {
			let mut current = scope.locked(&snapshot, &operation).await?;
			if current.revision != snapshot.revision || current.state != "frozen" {
				return Ok(false);
			}
			let sid = current.data["session_id"]
				.as_str()
				.ok_or(Error::Forbidden)?;
			let stopped = scope
				.request("POST", &format!("/v1/sessions/{sid}/stop"), None)
				.await?;
			if stopped["termination_confirmed"] != true {
				return Err(Error::Conflict("PYTHON_TERMINATION_UNCONFIRMED".into()));
			}
			current.state = "reset".into();
			current.data["reset_reason"] = json!(reason);
			scope.update(&mut current).await?;
			Ok(true)
		}
		.await;
		scope.finish(result).await?;
	}
	Ok(())
}

#[cfg(test)]
mod tests;
