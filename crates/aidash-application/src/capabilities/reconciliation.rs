//! Operation procedures authorize the worker, journal dispatch, and reconcile committed effects.
use crate::{
	Error, Result,
	ports::capabilities::reconciliation::{
		Loaded, OperationReconciliationRepository, OperationReconciliationScope,
	},
};
use aidash_domain::capabilities::operations::{
	FileScope, MountedFile, reconciliation::Snapshot, request_validation_failure,
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
		if !config.permits(&operation.kind) {
			return Err(Error::Forbidden);
		}
		scope.require_builtin(&operation.kind).await?;
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
		scope.set_area("uncertain").await?;
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
		scope
			.request("POST", &format!("/v1/operations/{id}/cancel"), None)
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
				"kind":if operation.kind=="code_interpreter"{"python"}else{"shell"},"code":operation.input["command"],"seconds":operation.input["seconds"],"files":files});
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
			scope.set_area("uncertain").await?;
		}
		Some("completed" | "cancelled" | "failed")
			if observed["termination_confirmed"] == true
				|| operation.kind == "code_interpreter" && observed["writer_frozen"] == true =>
		{
			if observed["executed"] == false {
				operation.state = observed["status"].as_str().unwrap().into();
				operation.result = json!({"termination_confirmed":true,"effects_may_have_occurred":false,"runner_acknowledged":false});
				scope.set_area("active").await?;
				return scope.persist(&operation).await;
			}
			let previous = serde_json::from_value::<Vec<MountedFile>>(area.manifest.clone())?;
			let mut entries = previous
				.iter()
				.filter(|file| !matches!(file.scope, FileScope::Working))
				.cloned()
				.collect::<Vec<_>>();
			let exported = observed["files"]
				.as_array()
				.ok_or_else(|| Error::External("invalid runner file manifest".into()))?;
			let mut size = entries.iter().map(|file| file.size).sum::<u64>();
			let mut seen = std::collections::BTreeSet::new();
			for file in exported {
				let path = file["path"].as_str().ok_or(Error::Forbidden)?;
				aidash_domain::registry::rules::validate_path(path)?;
				if !seen.insert(path) || entries.len() >= 4096 {
					return Err(Error::Invalid("invalid runner paths".into()));
				}
				let file_size = file["size"].as_u64().ok_or(Error::Forbidden)?;
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
				let (file_id, digest) = Box::pin(download_output(scope, id, file)).await?;
				entries.push(MountedFile {
					file_id,
					path: path.into(),
					digest,
					size: file_size,
					media_type: "application/octet-stream".into(),
					scope: FileScope::Working,
					provenance: json!({"kind":"operation","operation_id":id}),
				});
			}
			let stdout = STANDARD
				.decode(observed["stdout"].as_str().ok_or(Error::Forbidden)?)
				.map_err(|_| Error::Invalid("invalid runner output encoding".into()))?;
			if stdout.len() as u64 > limits.output_bytes {
				return Err(Error::Conflict("runner output quota".into()));
			}
			let (output_id, output_digest) = scope.put("output", &stdout).await?;
			let output = MountedFile {
				file_id: output_id,
				path: format!("operation-{id}.log"),
				digest: output_digest,
				size: stdout.len() as u64,
				media_type: "text/plain; charset=utf-8".into(),
				scope: FileScope::Working,
				provenance: json!({"kind":"operation","operation_id":id}),
			};
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
			if operation.kind == "code_interpreter" {
				scope.complete_python(&operation, &observed).await?;
			}
		}
		Some("uncertain") => {
			operation.state = "uncertain".into();
			operation.result = json!({"error":observed["error"],"termination_confirmed":observed["termination_confirmed"]});
			scope.set_area("uncertain").await?;
		}
		Some("accepted" | "starting" | "running" | "finishing") => {
			if operation.state != "cancelling" {
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
			let text = String::from_utf8_lossy(&bytes);
			let mut end = text.len().min(limits.read_bytes);
			while !text.is_char_boundary(end) {
				end -= 1;
			}
			operation.result = json!({"preview":&text[..end],"truncated":observed["truncated"]});
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
async fn download_output(
	scope: &mut dyn OperationReconciliationScope,
	operation: Uuid,
	file: &Value,
) -> Result<(Uuid, String)> {
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
		offset += bytes.len() as u64;
		scope.write_output(&bytes).await?;
	}
	scope.finish_output(expected).await
}
#[cfg(test)]
mod tests;
