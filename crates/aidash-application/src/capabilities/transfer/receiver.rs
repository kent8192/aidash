//! Thread Attachment deliveries reserve and verify bounded bytes before publication.
use crate::{
	Error, Result,
	ports::capabilities::transfer::receiver::{ReceiverRepository, ReceiverScope},
};
use aidash_domain::{
	capabilities::{
		operations::{FileScope, MountedFile as FileEntry},
		records::Record,
		transfer::{
			Chunk, Description, Identity,
			receiver::{chunk_digest, validate, view},
		},
	},
	federation::delivery::CHUNK_BYTES,
};
use base64::Engine;
use chrono::Utc;
use serde_json::{Value, json};

pub fn require_admission(repository: &dyn ReceiverRepository) -> Result<()> {
	if repository.admission_enabled() {
		Ok(())
	} else {
		Err(Error::Conflict("CAPABILITIES_DISABLED".into()))
	}
}

async fn finish<T>(scope: Box<dyn ReceiverScope + '_>, result: Result<T>) -> Result<T> {
	match result {
		Ok(value) => scope.finish(Ok(())).await.map(|()| value),
		Err(error) => match scope.finish(Err(error)).await {
			Err(error) => Err(error),
			Ok(()) => Err(Error::External(
				"delivery scope completion invariant".into(),
			)),
		},
	}
}

fn description<'a>(
	repository: &'a dyn ReceiverRepository,
	source: &str,
	input: &Identity,
) -> Result<&'a Description> {
	let description = repository.description();
	if description.protocol != "file-transfer/1"
		|| description.source_node != source
		|| description.target.node_id != repository.node_id()
		|| description.input_digest != input.input_digest
		|| description.transfer_id != input.transfer_id
	{
		return Err(Error::Forbidden);
	}
	Ok(description)
}

async fn current<'a>(
	repository: &'a dyn ReceiverRepository,
	source: &str,
	input: &Identity,
) -> Result<(Box<dyn ReceiverScope + 'a>, Record, &'a Description)> {
	let description = description(repository, source, input)?;
	let mut scope = repository.begin(source).await?;
	let record = scope.record(input.transfer_id).await?;
	if record.area_id.is_some()
		|| record.data["mapped_credential"] != json!(scope.credential())
		|| record.data["description"] != json!(description)
	{
		return finish(scope, Err(Error::Forbidden)).await;
	}
	Ok((scope, record, description))
}

pub async fn prepare(
	repository: &dyn ReceiverRepository,
	source: &str,
	input: Identity,
) -> Result<Value> {
	require_admission(repository)?;
	let description = description(repository, source, &input)?;
	let total = validate(description, &repository.limits(), Utc::now())?;
	let mut scope = repository.begin(source).await?;
	let result = async {
		match scope.record(input.transfer_id).await {
			Ok(record) => {
				if record.area_id.is_some()
					|| record.data["description"] != json!(description)
					|| record.data["mapped_credential"] != json!(scope.credential())
				{
					return Err(Error::Conflict("TRANSFER_IDENTITY_CHANGED".into()));
				}
				return Ok(view(&record));
			}
			Err(Error::NotFound(_)) => {}
			Err(error) => return Err(error),
		}
		// Reserve provisional chunks and the bytes needed to publish them.
		scope.lock_conversion().await?;
		let reserved = (2 * total) as i64;
		scope.reserve(reserved).await?;
		let data = json!({
			"description":description,
			"mapped_credential":scope.credential(),
			"chunks":{},
			"reserved":reserved,
		});
		let record = scope
			.insert(input.transfer_id, data, description.expires_at)
			.await?;
		Ok(view(&record))
	}
	.await;
	finish(scope, result).await
}

pub async fn chunk(
	repository: &dyn ReceiverRepository,
	source: &str,
	input: Chunk,
) -> Result<Value> {
	require_admission(repository)?;
	let identity = Identity {
		transfer_id: input.transfer_id,
		input_digest: input.input_digest,
	};
	let (mut scope, mut record, description) = current(repository, source, &identity).await?;
	let result = async {
		if record.state != "staging" || description.expires_at <= Utc::now() {
			return Err(Error::Conflict("TRANSFER_UNAVAILABLE".into()));
		}
		let file = description
			.files
			.get(input.file)
			.ok_or_else(|| Error::Invalid("TRANSFER_FILE_INDEX".into()))?;
		let bytes = base64::engine::general_purpose::STANDARD
			.decode(&input.data)
			.map_err(|_| Error::Invalid("INVALID_CHUNK".into()))?;
		if input.offset >= file.size
			|| !input.offset.is_multiple_of(CHUNK_BYTES)
			|| bytes.len() as u64 != (file.size - input.offset).min(CHUNK_BYTES)
		{
			return Err(Error::Invalid("TRANSFER_CHUNK_RANGE".into()));
		}
		let key = format!("{}:{}", input.file, input.offset);
		if let Some(old) = record.data["chunks"].get(&key) {
			if old["digest"] != chunk_digest(&bytes) || old["size"] != bytes.len() {
				return Err(Error::Conflict("CHUNK_CHANGED".into()));
			}
			return Ok(view(&record));
		}
		let reserved = record.data["reserved"].as_i64().ok_or(Error::Forbidden)?;
		if reserved < bytes.len() as i64 {
			return Err(Error::Conflict("TRANSFER_RESERVATION_CHANGED".into()));
		}
		scope.lock_conversion().await?;
		let (file_id, digest) = scope.put_staging(&bytes).await?;
		scope.reserve(-(bytes.len() as i64)).await?;
		record.data["chunks"][key] = json!(FileEntry {
			file_id,
			digest,
			path: "chunk".into(),
			size: bytes.len() as u64,
			media_type: "application/octet-stream".into(),
			scope: FileScope::Received,
			provenance: Value::Null
		});
		record.data["reserved"] = json!(reserved - bytes.len() as i64);
		scope.update(&mut record).await?;
		Ok(view(&record))
	}
	.await;
	finish(scope, result).await
}
/// A verified snapshot of the staged Thread Attachments.
pub enum Staged {
	/// The delivery already committed: its stored view.
	Committed(Value),
	Ready {
		description: Box<Description>,
		files: Vec<(FileEntry, Vec<u8>)>,
	},
}
/// Verify a delivery's complete bytes before the Home creates attachments.
pub async fn staged_files(
	repository: &dyn ReceiverRepository,
	source: &str,
	input: Identity,
) -> Result<Staged> {
	require_admission(repository)?;
	let (mut scope, record, description) = current(repository, source, &input).await?;
	let result = async {
		if record.state == "committed" {
			return Ok(Staged::Committed(view(&record)));
		}
		if record.state != "staging" || description.expires_at <= Utc::now() {
			return Err(Error::Conflict("TRANSFER_UNAVAILABLE".into()));
		}
		let mut files = Vec::with_capacity(description.files.len());
		for (index, file) in description.files.iter().enumerate() {
			let mut bytes = Vec::with_capacity(usize::try_from(file.size).unwrap_or(0));
			let mut offset = 0;
			while offset < file.size {
				let chunk: FileEntry = serde_json::from_value(
					record.data["chunks"][format!("{index}:{offset}")].clone(),
				)
				.map_err(|_| Error::Conflict("TRANSFER_INCOMPLETE".into()))?;
				bytes.extend_from_slice(&scope.read_file(&chunk).await?);
				offset += chunk.size;
			}
			if bytes.len() as u64 != file.size || chunk_digest(&bytes) != file.digest {
				return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
			}
			files.push((file.clone(), bytes));
		}
		Ok(Staged::Ready {
			description: Box::new(description.clone()),
			files,
		})
	}
	.await;
	finish(scope, result).await
}
/// Second half: record the delivery as committed under `receipt` and release what it reserved.
/// A repeat finds it committed and returns the stored view, never a second receipt.
pub async fn settle(
	repository: &dyn ReceiverRepository,
	source: &str,
	input: Identity,
	receipt: Value,
) -> Result<Value> {
	let (mut scope, mut record, _) = current(repository, source, &input).await?;
	let result = async {
		if record.state == "committed" {
			return Ok(view(&record));
		}
		if record.state != "staging" {
			return Err(Error::Conflict("TRANSFER_UNAVAILABLE".into()));
		}
		let reserved = record.data["reserved"].as_i64().ok_or(Error::Forbidden)?;
		scope.lock_conversion().await?;
		record.state = "committed".into();
		record.expires_at = None;
		record.data["receipt"] = receipt;
		record.data["reserved"] = json!(0);
		scope.update(&mut record).await?;
		// The chunks drew half of what `prepare` reserved; the rest is released with them.
		scope.reserve(-reserved).await?;
		Ok(view(&record))
	}
	.await;
	finish(scope, result).await
}

/// Manifest and bounded chunk references for streaming large Completion Message attachments.
pub enum Parts {
	Committed(Value),
	Ready {
		description: Box<Description>,
		files: Vec<Vec<FileEntry>>,
	},
}

pub async fn staged_parts(
	repository: &dyn ReceiverRepository,
	source: &str,
	input: Identity,
) -> Result<Parts> {
	require_admission(repository)?;
	let (scope, record, description) = current(repository, source, &input).await?;
	let result = (|| {
		if record.state == "committed" {
			return Ok(Parts::Committed(view(&record)));
		}
		if record.state != "staging" || description.expires_at <= Utc::now() {
			return Err(Error::Conflict("TRANSFER_UNAVAILABLE".into()));
		}
		let mut files = Vec::with_capacity(description.files.len());
		for (index, file) in description.files.iter().enumerate() {
			let mut parts = Vec::new();
			let mut offset = 0;
			while offset < file.size {
				let part: FileEntry = serde_json::from_value(
					record.data["chunks"][format!("{index}:{offset}")].clone(),
				)
				.map_err(|_| Error::Conflict("TRANSFER_INCOMPLETE".into()))?;
				if part.size == 0 || part.size > file.size - offset {
					return Err(Error::Conflict("TRANSFER_INCOMPLETE".into()));
				}
				offset += part.size;
				parts.push(part);
			}
			files.push(parts);
		}
		Ok(Parts::Ready {
			description: Box::new(description.clone()),
			files,
		})
	})();
	finish(scope, result).await
}
