//! Interpreter loss never replays old code; explicit acknowledgements admit a fresh session.
use crate::{
	Error, Result,
	ports::capabilities::python::{PythonRepository, PythonScope},
};
use aidash_domain::capabilities::sessions::Area;
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
		scope.confirm_stopped(&record).await?;
	}
	record.state = "reset".into();
	record.data["reset_reason"] = json!(reason);
	scope.update(&mut record).await
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
		stop_captured(repository, &snapshot, reason).await?;
	}
	Ok(())
}

#[cfg(test)]
mod tests;

/// Stop a captured heap through the same positive-confirmation owner as
/// idle/revoked maintenance. A running heap also requires its operation's durable
/// stop and receipt settlement; the caller fences Area reuse while pending.
pub async fn stop_captured(
	repository: &dyn PythonRepository,
	snapshot: &aidash_domain::capabilities::records::Record,
	reason: &str,
) -> Result<bool> {
	let operation = repository
		.operation(serde_json::from_value(
			snapshot.data["operation_id"].clone(),
		)?)
		.await?;
	let mut scope = repository.begin().await?;
	let result = async {
		let mut current = scope.locked(snapshot, &operation).await?;
		if current.revision != snapshot.revision
			|| current.state != snapshot.state
			|| !matches!(current.state.as_str(), "frozen" | "running")
			|| (current.state == "running" && !operation.writer_settled)
		{
			return Ok(false);
		}
		let sid = current.data["session_id"]
			.as_str()
			.ok_or(Error::Forbidden)?;
		let health = scope.request("GET", "/v1/health", None).await?;
		if current.data["instance"].as_str().is_none()
			|| health["instance"] != current.data["instance"]
		{
			return Err(Error::Conflict("PYTHON_RUNNER_CHANGED".into()));
		}
		let stopped = scope
			.request("POST", &format!("/v1/sessions/{sid}/stop"), None)
			.await?;
		if stopped["termination_confirmed"] != true {
			return Err(Error::Conflict("PYTHON_TERMINATION_UNCONFIRMED".into()));
		}
		scope.confirm_stopped(&current).await?;
		current.state = "reset".into();
		current.data["reset_reason"] = json!(reason);
		scope.update(&mut current).await?;
		Ok(true)
	}
	.await;
	let stopped = result.as_ref().copied().unwrap_or(false);
	scope.finish(result).await?;
	Ok(stopped)
}
