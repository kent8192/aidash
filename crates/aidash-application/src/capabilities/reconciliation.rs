//! Operation procedures authorize the worker, journal dispatch, and reconcile committed effects.
use crate::{
	Error, Result,
	ports::capabilities::reconciliation::{
		Loaded, OperationReconciliationRepository, OperationReconciliationScope,
	},
};
use aidash_domain::{
	capabilities::{
		errors::{CapabilityError, TimedOut, also_timed_out},
		operations::{
			ExportQuota, FileScope, MAX_AREA_FILES, MountedFile, VERIFICATION,
			WORKING_EXPORT_REFUSED, capability_of, counted_files, export_quota_refusal,
			lost_area_state,
			reconciliation::{Area, Limits, Snapshot},
			request_validation_failure, undelivered_refusal,
		},
	},
	completion::{MAX_DIAGNOSTIC_BYTES, bounded_tail},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
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
		mut area,
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
	if !cancelling && operation.kind == "python_install" {
		scope
			.authorize_packages(&operation.input["package_request"])
			.await?;
	}
	if cancelling && operation.state == "prepared" {
		operation.state = "cancelled".into();
		operation.result = json!({"termination_confirmed":true,"effects_may_have_occurred":false});
		scope.set_area("active").await?;
		if operation.kind == "code_interpreter" {
			scope
				.complete_python(&operation, &json!({"writer_frozen":false}))
				.await?;
		}
		return scope.persist(&operation).await;
	}
	let health = scope
		.health(cancelling, operation.kind == "code_interpreter")
		.await?;
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
			if operation.kind == "code_interpreter" {
				scope
					.complete_python(&operation, &json!({"writer_frozen":false}))
					.await?;
			}
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
		Some("completed" | "cancelled" | "failed")
			if observed["termination_confirmed"] == true
				|| operation.kind == "code_interpreter" && observed["writer_frozen"] == true =>
		{
			if observed["executed"] == false {
				operation.state = observed["status"].as_str().unwrap().into();
				operation.result = json!({"termination_confirmed":true,"effects_may_have_occurred":false,"runner_acknowledged":false});
				if !observed["error"].is_null() {
					operation.result["error"] = observed["error"].clone();
				}
				scope.set_area("active").await?;
				if operation.kind == "code_interpreter" {
					scope
						.complete_python(&operation, &json!({"writer_frozen":false}))
						.await?;
				}
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
			// The runner refused to export `/work` (#682): only this operation
			// fails, and the Area keeps its published revision.
			if observed["error"]["code"] == WORKING_EXPORT_REFUSED {
				let error = observed["error"].clone();
				return Box::pin(refuse_export(scope, &area, operation, &observed, error)).await;
			}
			let previous = serde_json::from_value::<Vec<MountedFile>>(area.manifest.clone())?;
			// A digest supplied by the runner is not authority to read an
			// arbitrary Tenant blob. Only the Area or this operation's mounted
			// inputs can authorize a download-free reuse.
			let mut entries = previous
				.iter()
				.filter(|file| !matches!(file.scope, FileScope::Working))
				.cloned()
				.collect::<Vec<_>>();
			let exported = observed["files"]
				.as_array()
				.ok_or_else(|| Error::External("invalid runner file manifest".into()))?;
			// The runner counts only `/work`; mounted inputs share the manifest.
			if let Some(error) = quota_refusal(&entries, exported, limits.working_bytes)? {
				return Box::pin(refuse_export(scope, &area, operation, &observed, error)).await;
			}
			let mounted = if exported.is_empty() {
				Vec::new()
			} else {
				scope.upload_files(&operation).await?
			};
			let mut size = counted_files(&entries).map(|file| file.size).sum::<u64>();
			let mut seen = std::collections::BTreeSet::new();
			for file in exported {
				let path = file["path"].as_str().ok_or(Error::Forbidden)?;
				aidash_domain::registry::rules::validate_path(path)?;
				if !seen.insert(path) || counted_files(&entries).count() >= MAX_AREA_FILES {
					return Err(Error::Invalid("invalid runner paths".into()));
				}
				let file_size = file["size"].as_u64().ok_or(Error::Forbidden)?;
				let object_id = file["object_id"]
					.as_str()
					.ok_or_else(|| Error::Invalid("invalid output id".into()))?;
				if object_id.len() != 64 || !object_id.bytes().all(|byte| byte.is_ascii_hexdigit())
				{
					return Err(Error::Invalid("invalid output id".into()));
				}
				let digest = file["digest"]
					.as_str()
					.ok_or_else(|| Error::Invalid("invalid output digest".into()))?;
				digest
					.parse::<aidash_domain::capabilities::Digest>()
					.map_err(|_| Error::Invalid("invalid output digest".into()))?;
				size = size.checked_add(file_size).ok_or(Error::Forbidden)?;
				if size > limits.working_bytes {
					return Err(Error::Conflict("runner file quota".into()));
				}
				if let Some(unchanged) = previous.iter().find(|old| {
					matches!(old.scope, FileScope::Working)
						&& old.path == path
						&& old.size == file_size
						&& file["digest"] == old.digest
				}) {
					scope.verified(unchanged).await?;
					entries.push(unchanged.clone());
					continue;
				}
				// Reuse an output blob only when this Area already held it, keeping
				// the media type that was detected when it was first downloaded.
				let authorized = previous
					.iter()
					.chain(mounted.iter())
					.find(|file| file.digest == digest && file.size == file_size)
					.map(|file| file.media_type.clone());
				let reusable = match authorized {
					Some(media_type) => scope
						.reuse_output(digest, file_size)
						.await?
						.map(|file_id| (file_id, media_type)),
					None => None,
				};
				let (file_id, digest, media_type) = match reusable {
					Some((file_id, media_type)) => (file_id, digest.to_owned(), media_type),
					None => Box::pin(download_output(scope, id, file)).await?,
				};
				entries.push(MountedFile {
					file_id,
					path: path.into(),
					digest,
					size: file_size,
					media_type,
					scope: FileScope::Working,
					provenance: json!({"kind":"operation","operation_id":id}),
				});
			}
			let output = store_output(scope, id, &observed, limits).await?;
			let mut displays = vec![];
			let mut display_bytes = 0;
			for (index, display) in observed["displays"]
				.as_array()
				.map(Vec::as_slice)
				.unwrap_or_default()
				.iter()
				.enumerate()
			{
				if index >= 16 || display["mime"] != "image/png" {
					return Err(Error::Invalid("DISPLAY_LIMIT".into()));
				}
				let bytes = STANDARD
					.decode(display["data"].as_str().ok_or(Error::Forbidden)?)
					.map_err(|_| Error::Invalid("INVALID_DISPLAY".into()))?;
				display_bytes += bytes.len();
				if display_bytes as u64 > limits.output_bytes
					|| !bytes.starts_with(b"\x89PNG\r\n\x1a\n")
				{
					return Err(Error::Invalid("DISPLAY_LIMIT".into()));
				}
				let (file_id, digest) = scope.put("display", &bytes).await?;
				displays.push(MountedFile {
					file_id,
					path: format!("display-{id}-{index}.png"),
					digest,
					size: bytes.len() as u64,
					media_type: "image/png".into(),
					scope: FileScope::Working,
					provenance: json!({"kind":"display","operation_id":id}),
				});
			}
			area.revision = scope.publish(entries).await?;
			scope.set_area("active").await?;
			operation.revision = area.revision;
			operation.state = observed["status"].as_str().unwrap().into();
			operation.result = json!({"displays":displays,"session_id":operation.input["session_id"],"writer_frozen":observed["writer_frozen"],"output_file":output,"exit_code":observed["exit_code"],"truncated":observed["truncated"],"termination_confirmed":observed["termination_confirmed"],"runner_acknowledged":false,"error":observed["error"]});
			// The limit decides, whatever exit code the command then gave and
			// whoever stopped it. Its files are published by now, so the error
			// can say they were saved.
			if observed["deadline_expired"] == true {
				operation.state = "failed".into();
				operation.result["deadline_expired"] = json!(true);
				operation.result["error"] =
					serde_json::to_value(CapabilityError::command_timed_out(&TimedOut {
						limit_seconds: time_limit(&operation),
						maximum_seconds: scope
							.timeout_ceiling(&operation.kind)
							.unwrap_or(limits.maximum_seconds)
							.min(limits.maximum_seconds),
						revision: area.revision,
						python: operation.kind == "code_interpreter",
						install: operation.kind == "python_install",
					}))?;
			}
			if operation.kind == "code_interpreter" {
				scope.complete_python(&operation, &observed).await?;
			}
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
			if !held && operation.kind == "code_interpreter" {
				scope
					.complete_python(&operation, &json!({"writer_frozen":false}))
					.await?;
			}
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
/// Saves the operation's command output as its own object, outside the Area
/// manifest.
async fn store_output(
	scope: &mut dyn OperationReconciliationScope,
	id: Uuid,
	observed: &Value,
	limits: Limits,
) -> Result<MountedFile> {
	let stdout = STANDARD
		.decode(observed["stdout"].as_str().ok_or(Error::Forbidden)?)
		.map_err(|_| Error::Invalid("invalid runner output encoding".into()))?;
	if stdout.len() as u64 > limits.output_bytes {
		return Err(Error::Conflict("runner output quota".into()));
	}
	let (file_id, digest) = scope.put("output", &stdout).await?;
	Ok(MountedFile {
		file_id,
		path: format!("operation-{id}.log"),
		digest,
		size: stdout.len() as u64,
		media_type: "text/plain; charset=utf-8".into(),
		scope: FileScope::Working,
		provenance: json!({"kind":"operation","operation_id":id}),
	})
}
/// The refusal of an export that would exceed the manifest's file ceiling or
/// the byte quota, naming the first exported file over it.
fn quota_refusal(
	entries: &[MountedFile],
	exported: &[Value],
	working_bytes: u64,
) -> Result<Option<Value>> {
	let mounted = counted_files(entries).map(|file| file.size).sum::<u64>();
	let counted = counted_files(entries).count();
	let mut size = mounted;
	for (index, file) in exported.iter().enumerate() {
		let path = file["path"].as_str().ok_or(Error::Forbidden)?;
		if counted + index >= MAX_AREA_FILES {
			return Ok(Some(export_quota_refusal(
				ExportQuota::Files,
				path,
				MAX_AREA_FILES as u64,
			)));
		}
		size = size.saturating_add(file["size"].as_u64().ok_or(Error::Forbidden)?);
		if size > working_bytes {
			return Ok(Some(export_quota_refusal(
				ExportQuota::Bytes,
				path,
				working_bytes.saturating_sub(mounted),
			)));
		}
	}
	Ok(None)
}
/// The Time Limit the operation was started with.
fn time_limit(operation: &Snapshot) -> u64 {
	operation.input["seconds"].as_u64().unwrap_or_default()
}
/// Ends an operation whose export was refused as a failed tool call: nothing is
/// published, the Area reopens at its previous revision and the Run continues.
/// A Python session resets, since its memory no longer matches those files.
/// A command past its limit keeps the refusal as its error, because what the
/// Agent has to act on first is that nothing was saved, and adds the limit.
async fn refuse_export(
	scope: &mut dyn OperationReconciliationScope,
	area: &Area,
	mut operation: Snapshot,
	observed: &Value,
	mut error: Value,
) -> Result<()> {
	let id = operation.id;
	let timed_out = observed["deadline_expired"] == true;
	if timed_out && let Some(message) = error["message"].as_str() {
		error["message"] = json!(also_timed_out(message, time_limit(&operation)));
	}
	let limits = scope.limits();
	let output = store_output(scope, id, observed, limits).await?;
	scope.set_area("active").await?;
	operation.revision = area.revision;
	operation.state = "failed".into();
	operation.result = json!({"session_id":operation.input["session_id"],"writer_frozen":false,"output_file":output,"exit_code":observed["exit_code"],"truncated":observed["truncated"],"termination_confirmed":true,"runner_acknowledged":false,"error":error});
	if timed_out {
		operation.result["deadline_expired"] = json!(true);
	}
	if operation.kind == "code_interpreter" {
		scope
			.complete_python(&operation, &json!({"writer_frozen":false}))
			.await?;
	}
	scope.persist(&operation).await?;
	scope
		.event(json!({"operation_id":id,"area_id":area.id,"status":operation.state}))
		.await
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
pub(super) async fn download_output<
	T: crate::ports::capabilities::processing::OutputTransfer + ?Sized,
>(
	scope: &mut T,
	operation: Uuid,
	file: &Value,
) -> Result<(Uuid, String, String)> {
	let size = file["size"]
		.as_u64()
		.ok_or_else(|| Error::Invalid("invalid output size".into()))?;
	let id = file["object_id"]
		.as_str()
		.ok_or_else(|| Error::Invalid("invalid output id".into()))?;
	if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
		return Err(Error::Invalid("invalid output id".into()));
	}
	let expected = file["digest"]
		.as_str()
		.ok_or_else(|| Error::Invalid("invalid output digest".into()))?;
	scope.begin_output(size).await?;
	let mut offset = 0;
	let mut prefix = Vec::with_capacity(12);
	while offset < size {
		let chunk = scope
			.request(
				"GET",
				&format!("/v1/operations/{operation}/files/{id}?offset={offset}"),
				None,
			)
			.await?;
		let bytes = STANDARD
			.decode(chunk["data"].as_str().ok_or(Error::Forbidden)?)
			.map_err(|_| Error::Invalid("invalid output chunk".into()))?;
		if bytes.is_empty()
			|| bytes.len() > 4 << 20
			|| chunk["offset"] != offset
			|| chunk["size"] != size
			|| chunk["digest"] != expected
		{
			return Err(Error::Conflict("output chunk integrity".into()));
		}
		let needed = 12_usize.saturating_sub(prefix.len());
		prefix.extend_from_slice(&bytes[..bytes.len().min(needed)]);
		offset += bytes.len() as u64;
		scope.write_output(&bytes).await?;
	}
	let (id, digest) = scope.finish_output(expected).await?;
	let media_type = aidash_domain::provider::detect_image(&prefix)
		.unwrap_or("application/octet-stream")
		.to_owned();
	Ok((id, digest, media_type))
}
#[cfg(test)]
mod tests;
