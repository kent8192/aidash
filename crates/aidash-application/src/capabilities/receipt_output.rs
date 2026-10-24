//! Retain a stopped uncertain result without publishing successful Area effects.
use crate::{Error, Result, ports::capabilities::processing::OperationProcessingRepository};
use aidash_domain::capabilities::operations::processing::{Receipt, output_digest};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

pub(super) async fn adopt(
	repository: &dyn OperationProcessingRepository,
	receipt: &Receipt,
) -> Result<Option<String>> {
	let observed = repository
		.runner_request("GET", &format!("/v1/operations/{}", receipt.id), None)
		.await?;
	if observed["operation_id"] != json!(receipt.id)
		|| observed["digest"] != receipt.digest
		|| observed["status"] != "uncertain"
		|| observed["termination_confirmed"] != true
	{
		return Err(Error::Forbidden);
	}
	let instance = receipt.runner_instance.as_deref().ok_or(Error::Forbidden)?;
	let mut scope = repository.begin_receipt_output(receipt).await?;
	// An acknowledged retry retains its original proof even after payload drain.
	if observed["acknowledged"] == true {
		let proof = observed["acknowledged_output_digest"]
			.as_str()
			.ok_or(Error::Forbidden)?;
		scope.commit(proof, Value::Null).await?;
		return Ok(Some(proof.to_owned()));
	}
	if observed["output_harvested"] != true {
		return Ok(None);
	}
	let proof = output_digest(instance, &observed)?;
	if observed["output_digest"] != proof {
		return Err(Error::Forbidden);
	}
	let limits = scope.limits();
	let files = observed["files"].as_array().ok_or(Error::Forbidden)?;
	if files.len() > 4096 {
		return Err(Error::Invalid("invalid runner paths".into()));
	}
	let mut total = 0u64;
	let mut paths = std::collections::BTreeSet::new();
	for file in files {
		let path = file["path"].as_str().ok_or(Error::Forbidden)?;
		aidash_domain::registry::rules::validate_path(path)?;
		if !paths.insert(path) {
			return Err(Error::Invalid("invalid runner paths".into()));
		}
		let size = file["size"].as_u64().ok_or(Error::Forbidden)?;
		total = total.checked_add(size).ok_or(Error::Forbidden)?;
		if total > limits.working_bytes {
			return Err(Error::Conflict("runner file quota".into()));
		}
	}
	let mut retained = vec![];
	for file in files {
		let path = file["path"].as_str().ok_or(Error::Forbidden)?;
		let size = file["size"].as_u64().ok_or(Error::Forbidden)?;
		let (id, digest) =
			super::reconciliation::download_output(&mut *scope, receipt.id, file).await?;
		retained.push(json!({"file_id":id,"path":path,"digest":digest,"size":size,"media_type":"application/octet-stream",
			"scope":"working","provenance":{"kind":"stopped_operation","operation_id":receipt.id}}));
	}
	let encoded_stdout = observed["stdout"].as_str().ok_or(Error::Forbidden)?;
	check_encoding_budget(encoded_stdout, limits.output_bytes)?;
	let stdout = STANDARD
		.decode(encoded_stdout)
		.map_err(|_| Error::Invalid("invalid runner output encoding".into()))?;
	if stdout.len() as u64 > limits.output_bytes {
		return Err(Error::Conflict("runner output quota".into()));
	}
	let (id, digest) = scope.put("output", &stdout).await?;
	let output = json!({"file_id":id,"path":format!("operation-{}.log", receipt.id),"digest":digest,"size":stdout.len(),
		"media_type":"text/plain; charset=utf-8","scope":"working","provenance":{"kind":"stopped_operation","operation_id":receipt.id}});
	let mut displays = vec![];
	let mut display_bytes = 0u64;
	let no_displays = vec![];
	let observed_displays = match observed.get("displays") {
		None => &no_displays, // Existing shell journals omit this Python-only field.
		Some(displays) => displays.as_array().ok_or(Error::Forbidden)?,
	};
	for (index, display) in observed_displays.iter().enumerate() {
		if index >= 16 || display["mime"] != "image/png" {
			return Err(Error::Invalid("DISPLAY_LIMIT".into()));
		}
		let encoded = display["data"].as_str().ok_or(Error::Forbidden)?;
		check_encoding_budget(encoded, limits.output_bytes - display_bytes)?;
		let bytes = STANDARD
			.decode(encoded)
			.map_err(|_| Error::Invalid("INVALID_DISPLAY".into()))?;
		display_bytes = display_bytes
			.checked_add(bytes.len() as u64)
			.ok_or(Error::Forbidden)?;
		if display_bytes > limits.output_bytes || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
			return Err(Error::Invalid("DISPLAY_LIMIT".into()));
		}
		let (id, digest) = scope.put("display", &bytes).await?;
		displays.push(json!({"file_id":id,"path":format!("display-{}-{index}.png", receipt.id),"digest":digest,"size":bytes.len(),
			"media_type":"image/png","scope":"working","provenance":{"kind":"stopped_operation","operation_id":receipt.id}}));
	}
	let health = repository.runner_request("GET", "/v1/health", None).await?;
	if health["instance"] != instance {
		return Err(Error::Forbidden);
	}
	scope.commit(&proof, json!({"files":retained,"output_file":output,"displays":displays,"truncated":observed["truncated"]})).await?;
	Ok(Some(proof))
}

fn check_encoding_budget(encoded: &str, limit: u64) -> Result<()> {
	let maximum = limit
		.checked_add(2)
		.and_then(|n| (n / 3).checked_mul(4))
		.ok_or(Error::Forbidden)?;
	if encoded.len() as u64 > maximum {
		return Err(Error::Conflict("runner output quota".into()));
	}
	Ok(())
}
