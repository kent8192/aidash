//! Operation procedures authorize the worker, journal dispatch, and reconcile committed effects.
use crate::{
	Error, Result,
	ports::capabilities::reconciliation::{
		Loaded, OperationReconciliationRepository, OperationReconciliationScope,
	},
};
use aidash_domain::{
	capabilities::operations::{
		VERIFICATION, capability_of, lost_area_state, reconciliation::Snapshot,
		request_validation_failure, undelivered_refusal,
	},
	completion::{MAX_DIAGNOSTIC_BYTES, bounded_tail},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::json;
use uuid::Uuid;
pub async fn drive(repository: &dyn OperationReconciliationRepository, id: Uuid) -> Result<()> {
	let snapshot = repository.snapshot(id).await?;
	let mut scope = match repository.begin(&snapshot).await {
		Ok(scope) => scope,
		Err(Error::Forbidden | Error::Unauthorized | Error::NotFound(_)) => {
			return repository.withdraw(&snapshot).await;
		}
		Err(error) => return Err(error),
	};
	let outcome = Box::pin(reconcile(&mut *scope, &snapshot, id)).await;
	let result = scope.finish(outcome).await;
	match result {
		Err(Error::Forbidden | Error::Unauthorized | Error::NotFound(_)) => {
			repository.withdraw(&snapshot).await
		}
		other => other,
	}
}
async fn reconcile(
	scope: &mut dyn OperationReconciliationScope,
	snapshot: &Snapshot,
	id: Uuid,
) -> Result<()> {
	let Loaded {
		run,
		area,
		mut operation,
	} = scope.load(id, snapshot.run_id).await?;
	if !operation.active() {
		return Ok(());
	}
	if !operation.writer_matches(&area) {
		return Err(Error::Forbidden);
	}
	let limits = scope.limits();
	let cancelling = operation.cancelling(limits, &run);
	if !cancelling {
		let config = scope.configuration().await?;
		// A Check runs under the authority of the bound shell operation.
		let capability = capability_of(&operation.kind);
		if !config.permits(capability) {
			return Err(Error::Forbidden);
		}
		scope.require_builtin(capability).await?;
	}
	if cancelling && operation.state == "prepared" {
		operation.state = "cancelled".into();
		operation.result = json!({"termination_confirmed":true,"effects_may_have_occurred":false});
		scope.set_area("active").await?;
		return scope.persist(&operation).await;
	}
	let health = scope.health(cancelling, false).await?;
	let instance = health["instance"]
		.as_str()
		.ok_or_else(|| Error::External("invalid runner identity".into()))?;
	let identity_changed = operation.runner_changed(instance, cancelling);
	if identity_changed {
		operation.state = "uncertain".into();
		operation.result = json!({"error":"runner journal identity changed"});
		// A lost journal cannot prove a shell writer stopped; a Check never exports.
		scope
			.set_area(lost_area_state(
				&operation.kind,
				operation.kind != VERIFICATION,
			))
			.await?;
		return scope.persist(&operation).await;
	}
	if operation.state == "prepared" {
		operation.runner_instance = Some(instance.into());
		operation.state = "submitted".into();
		scope.persist(&operation).await?;
		// Commit the attempted operation before handing it to another process.
		// The next pass either observes the UUID or submits the recorded bytes.
		operation.result = json!({"dispatch_pending":true});
		return scope.persist(&operation).await;
	}
	let observed = if cancelling {
		operation.state = "cancelling".into();
		let refusal = undelivered_refusal(
			&operation.result,
			&operation.digest,
			operation.runner_instance.as_deref(),
		);
		scope
			.request("POST", &format!("/v1/operations/{id}/cancel"), refusal)
			.await?
	} else if operation.result["dispatch_pending"] == true {
		let files = scope.dispatch_files(&operation).await?;
		if let Some((code, message)) = request_validation_failure(limits.working_bytes, &files) {
			operation.state = "failed".into();
			operation.result = json!({"error":{"code":code,"message":message},"termination_confirmed":true,"effects_may_have_occurred":false,"runner_acknowledged":true});
			scope.set_area("active").await?;
			return scope.persist(&operation).await;
		}
		let files = files.into_iter().map(|file| json!({"file_id":file.file_id,"path":file.path,"digest":file.digest,"scope":file.scope,"size":file.size})).collect::<Vec<_>>();
		let mut input = json!({"operation_id":operation.id,"area_id":area.id,"epoch":operation.epoch,"digest":operation.digest,
				"kind":match operation.kind.as_str(){"code_interpreter"=>"python",VERIFICATION=>"verify",_=>"shell"},"code":operation.input["command"],"seconds":operation.input["seconds"],"files":files});
		if operation.kind == "code_interpreter" {
			input["session_id"] = operation.input["session_id"].clone();
		}
		scope.request("POST", "/v1/operations", Some(input)).await?
	} else {
		scope
			.request("GET", &format!("/v1/operations/{id}"), None)
			.await?
	};
	match observed["status"].as_str() {
		Some("awaiting_files") => {
			Box::pin(upload_inputs(scope, &operation)).await?;
			scope
				.request("POST", &format!("/v1/operations/{id}/start"), None)
				.await?;
			operation.state = "submitted".into();
			operation.result = json!({});
		}
		Some("absent") => {
			operation.state = "uncertain".into();
			operation.result = json!({"error":"runner lost a previously accepted operation"});
			// A lost journal cannot prove a shell writer stopped; a Check never exports.
			scope
				.set_area(lost_area_state(
					&operation.kind,
					operation.kind != VERIFICATION,
				))
				.await?;
		}
		Some("completed" | "cancelled" | "failed") if observed["termination_confirmed"] == true => {
			if observed["executed"] == false {
				operation.state = observed["status"].as_str().unwrap().into();
				operation.result = json!({"termination_confirmed":true,"effects_may_have_occurred":false,"runner_acknowledged":false});
				if !observed["error"].is_null() {
					operation.result["error"] = observed["error"].clone();
				}
				scope.set_area("active").await?;
				return scope.persist(&operation).await;
			}

			if operation.kind == VERIFICATION {
				// A Check never changes the Working Area: nothing is published and
				// the revision stays. Only the tail of its output is kept.
				let stdout = STANDARD
					.decode(observed["stdout"].as_str().ok_or(Error::Forbidden)?)
					.map_err(|_| Error::Invalid("invalid runner output encoding".into()))?;
				let text = String::from_utf8_lossy(&stdout);
				scope.set_area("active").await?;
				operation.revision = area.revision;
				operation.state = observed["status"].as_str().unwrap().into();
				operation.result = json!({"preview":bounded_tail(&text, MAX_DIAGNOSTIC_BYTES),"exit_code":observed["exit_code"],"truncated":observed["truncated"],"termination_confirmed":observed["termination_confirmed"],"runner_acknowledged":false,"error":observed["error"]});
				// Whether the Check's deadline stopped the command decides a
				// timeout; the runtime below is only a bound for charging.
				if let Some(expired) = observed["deadline_expired"].as_bool() {
					operation.result["deadline_expired"] = json!(expired);
				}
				// The runner's own measure of the command, unlike the time until
				// the Completion Gate harvests this result: it ends when the
				// runner saw the command stop, before it collected the output.
				// Milliseconds keep a sub-second Check's runtime, so the gate can
				// both charge it and tell a quick failure from a timeout.
				if let (Some(started), Some(finished)) = (
					observed["started_at"].as_f64(),
					observed["terminated_at"]
						.as_f64()
						.or_else(|| observed["finished_at"].as_f64()),
				) && finished >= started
				{
					operation.result["runtime_millis"] =
						json!(((finished - started) * 1000.0).ceil() as u64);
				}
				scope.persist(&operation).await?;
				return scope
					.event(json!({"operation_id":id,"area_id":area.id,"status":operation.state}))
					.await;
			}
			// Agent commands and Python cells run in Execution Pods (#745); a
			// Check is the only command left here, and nothing is published for
			// anything else.
			scope.set_area("active").await?;
			operation.state = "failed".into();
			operation.result = json!({"termination_confirmed":true,"runner_acknowledged":false,
				"error":{"code":"EXECUTION_PATH_REMOVED","message":"This command ran on the Runner's per-command path, which no longer publishes files.","retryable":false}});
		}
		Some("uncertain") => {
			// The runner refuses a successor while this writer is not confirmed
			// stopped, so a Check must not reopen the Area to one.
			let held = observed["termination_confirmed"] != true;
			// A Check exported nothing, so once its writer is proven stopped the
			// Area is usable again. Cancelling keeps it polled: the runner then
			// retries the stop proof instead of fencing the Area for good.
			operation.state = if held && operation.kind == VERIFICATION {
				"cancelling"
			} else {
				"uncertain"
			}
			.into();
			operation.result = json!({"error":observed["error"],"termination_confirmed":observed["termination_confirmed"]});
			scope
				.set_area(lost_area_state(&operation.kind, held))
				.await?;
		}
		// `cancelling` is the runner retrying a held writer's stop proof.
		Some("accepted" | "starting" | "running" | "finishing" | "cancelling") => {
			if observed["status"] == "cancelling" {
				operation.state = "cancelling".into();
			} else if operation.state != "cancelling" {
				operation.state =
					if matches!(observed["status"].as_str(), Some("accepted" | "starting")) {
						"submitted"
					} else {
						"running"
					}
					.into();
			}
			let bytes = STANDARD
				.decode(observed["stdout"].as_str().unwrap_or_default())
				.map_err(|_| Error::Invalid("invalid runner output preview".into()))?;
			if bytes.len() > 16384 {
				return Err(Error::Invalid("runner preview limit".into()));
			}
			let mut start = bytes.len().saturating_sub(limits.read_bytes);
			while !super::projection::boundary(&bytes, start) {
				start += 1;
			}
			let offset = observed["stdout_offset"]
				.as_u64()
				.unwrap_or(0)
				.checked_add(start as u64)
				.ok_or_else(|| Error::Invalid("invalid runner output offset".into()))?;
			let preview = &bytes[start..];
			operation.result = json!({"preview":String::from_utf8_lossy(preview),"preview_bytes":STANDARD.encode(preview),"preview_offset":offset,"truncated":observed["truncated"]});
			// The Time Limit counts from the command's start, which only the
			// runner knows. A Python cell's start is when it was dispatched.
			if let Some(started) = observed["started_at"]
				.as_f64()
				.or_else(|| observed["running_at"].as_f64())
			{
				operation.result["started_at"] = json!(started);
			}
			let error = if observed["status"] == "cancelling"
				&& observed["termination_confirmed"] != true
			{
				json!({"code":"STOP_UNCONFIRMED","message":"The command's stop is unconfirmed. Saved files can be read; new commands receive AREA_BUSY. You may decide what to do next.","retryable":true})
			} else {
				observed["error"].clone()
			};
			if !observed["output_fetched_at"].is_null() {
				operation.result["output_fetched_at"] = observed["output_fetched_at"].clone();
			}
			if observed["termination_confirmed"] == true {
				operation.result["termination_confirmed"] = json!(true);
			}
			if !error.is_null() {
				operation.result["error"] = error;
			}
		}
		_ => return Err(Error::External("invalid runner operation state".into())),
	}
	scope.persist(&operation).await?;
	scope
		.event(json!({"operation_id":id,"area_id":area.id,"status":operation.state}))
		.await?;
	Ok(())
}
async fn upload_inputs(
	scope: &mut dyn OperationReconciliationScope,
	operation: &Snapshot,
) -> Result<()> {
	let id = operation.id;
	for file in scope.upload_files(operation).await? {
		let mut offset = 0;
		loop {
			let bytes = scope.read_chunk(&file, offset).await?;
			let length = bytes.len() as u64;
			if length == 0 && offset < file.size {
				return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
			}
			scope
				.request(
					"POST",
					&format!(
						"/v1/operations/{id}/inputs/{}?offset={offset}",
						file.file_id
					),
					Some(json!({"data":STANDARD.encode(bytes)})),
				)
				.await?;
			offset += length;
			if offset == file.size {
				break;
			}
		}
	}
	Ok(())
}
#[cfg(test)]
mod tests;
