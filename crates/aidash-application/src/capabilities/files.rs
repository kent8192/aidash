//! File tools share current authority, immutable content and generation-bound search cursors.
use super::pod::{files as pod_files, save as pod_save};
use crate::{
	Error, Result,
	ports::capabilities::{
		files::{
			Action, FileScopePort, ScratchPublisher, ScratchStager, ScratchUpload, VerifiedFile,
		},
		pod_operations::PodContext,
	},
};
use aidash_domain::{
	RunMetadata,
	capabilities::{
		errors::CapabilityError,
		files::*,
		operations::{FileScope, MountedFile as FileEntry, available},
		sessions::Area,
	},
	model::ModelConfig,
	provider::ContentPart,
	registry::{AgentConfig, EntityRef},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use uuid::Uuid;
pub(crate) fn files(area: &Area) -> Result<Vec<FileEntry>> {
	Ok(serde_json::from_value(area.manifest.clone())?)
}
/// The Reference record a Reference file of the Area was taken from. A malformed
/// id names no record, so such a file is never usable.
fn reference_of(file: &FileEntry) -> Option<Uuid> {
	file.is_reference_file().then(|| {
		file.provenance["id"]
			.as_str()
			.and_then(|id| id.parse().ok())
			.unwrap_or_else(Uuid::nil)
	})
}
/// An Area holds a Reference file only while its record can be used: the manifest
/// keeps the entry after a revocation, so the file tools check the record (ADR 0114).
pub(crate) async fn require_usable(scope: &mut dyn FileScopePort, file: &FileEntry) -> Result<()> {
	let Some(id) = reference_of(file) else {
		return Ok(());
	};
	if scope.unusable_references(&[id]).await?.is_empty() {
		Ok(())
	} else {
		Err(Error::NotFound(format!(
			"reference unavailable: {}",
			file.path
		)))
	}
}
pub async fn settings(scope: &mut dyn FileScopePort, run: &RunMetadata) -> Result<AgentConfig> {
	let entry = scope
		.entry(
			&EntityRef {
				id: run.agent_id.clone(),
				version: run.agent_version.clone(),
			},
			"agent.execute",
		)
		.await?;
	scope.check_pinned(&entry).await?;
	Ok(AgentConfig::from_snapshot(scope.binding_snapshot()?)?)
}
pub async fn output_file(
	scope: &mut dyn FileScopePort,
	area: &Area,
	id: Uuid,
) -> Result<FileEntry> {
	let row: Option<(String, i64, String)> = scope.output(area, id).await?;
	let (digest, size, kind) = row.ok_or_else(|| Error::NotFound("file unavailable".into()))?;
	Ok(FileEntry {
		file_id: id,
		path: id.to_string(),
		digest,
		size: size as u64,
		scope: FileScope::Working,
		media_type: match kind.as_str() {
			"output" => "text/plain; charset=utf-8",
			"display" => "image/png",
			_ => "application/octet-stream",
		}
		.into(),
		provenance: json!({"kind":kind,"area_id":area.id}),
	})
}
/// Current authority to invoke the Run's bound operation `name` with `input`,
/// narrowed by its Binding. The admitted definition must be unchanged.
async fn admit(
	scope: &mut dyn FileScopePort,
	run: &RunMetadata,
	name: &str,
	input: Value,
) -> Result<Value> {
	settings(scope, run).await?;
	let binding = scope.binding_snapshot()?.operation(name)?.clone();
	if binding.identity.registry_deployment != scope.local_node() {
		return Err(Error::Forbidden);
	}
	let current = scope
		.entry(&binding.identity.local(), "registry.read")
		.await?;
	scope.check_pinned(&current).await?;
	if aidash_domain::registry::rules::digest(&serde_json::to_value(&current)?) != binding.digest {
		return Err(Error::Conflict(
			"admitted Binding definition changed".into(),
		));
	}
	let mut input = input;
	binding.narrow.apply(&mut input)?;
	scope
		.require(
			&scope.resource(
				"tool",
				&binding.identity.resource_id(),
				crate::authorization::catalog::attributes(&current),
			),
			"tool.invoke",
		)
		.await?;
	Ok(input)
}
/// The same admission a bound tool call passes before dispatch: current
/// settings, the Run's Binding, the pin and the definition digest, then
/// `tool.invoke` with the Binding's narrowing applied. A Live preview start
/// must pass it exactly as a Shell operation does (ADR 0034).
pub async fn admit_tool(
	scope: &mut dyn FileScopePort,
	run: &RunMetadata,
	name: &str,
	input: Value,
) -> Result<Value> {
	admit(scope, run, name, input).await
}

/// Start one Completion Gate Check (#112). It passes the same Binding,
/// narrowing and `tool.invoke` checks as the model's own shell calls; only the
/// operation kind differs, so the Check's files never reach the Working Area.
pub async fn verify(
	scope: &mut dyn FileScopePort,
	run: &RunMetadata,
	input: Value,
	key: &str,
) -> Result<Value> {
	let input = admit(scope, run, VERIFICATION_CAPABILITY, input).await?;
	let mut area = scope.for_run(run).await?;
	scope.execute(Action::Verify, &mut area, input, key).await
}
/// The current result of a Check this Run started.
pub async fn verification(
	scope: &mut dyn FileScopePort,
	run: &RunMetadata,
	operation_id: Uuid,
	key: &str,
) -> Result<Value> {
	let mut area = scope.for_run(run).await?;
	scope
		.execute(
			Action::Control {
				kind: aidash_domain::capabilities::operations::VERIFICATION,
				cancel: false,
			},
			&mut area,
			json!({"operation_id": operation_id}),
			key,
		)
		.await
}
const VERIFICATION_CAPABILITY: &str = "shell";
pub async fn invoke(
	scope: &mut dyn FileScopePort,
	run: &RunMetadata,
	name: &str,
	input: Value,
	key: &str,
) -> Result<Invocation> {
	let input = admit(scope, run, name, input).await?;
	if name == "file_share" {
		scope.serialize_sharing().await?;
	}
	let mut area = scope.for_run(run).await?;
	// Tools on the Scratch Space reach the Run's Execution Pod (#745).
	let scratch = aidash_domain::tool::concurrency::scratch_file_read(name, &input);
	let pod = if scratch || POD_TOOLS.contains(&name) || name == "apply_patch" {
		scope.pod(run).await?
	} else {
		None
	};
	if pod.is_none() && (scratch || (POD_TOOLS.contains(&name) && name != "file_remove")) {
		return Err(Error::Conflict(
			"RUNTIME_UNAVAILABLE: this Deployment runs no Execution Pods".into(),
		));
	}
	// A running writer has not published its changes; this manifest is the last saved revision.
	let saved_read = area.state == "running" && matches!(name, "file_read" | "file_search");
	// Only tools that publish a Working Area revision need it free.
	let scratch_only = scratch
		|| (pod.is_some() && name == "apply_patch")
		|| matches!(
			name,
			"file_copy" | "process_list" | "process_log" | "process_stop"
		);
	if !saved_read
		&& !scratch_only
		&& !matches!(
			name,
			"shell"
				| "shell_poll"
				| "shell_cancel"
				| "code_interpreter"
				| "python_poll"
				| "python_cancel"
		) {
		available(&area.state)?;
	}
	if matches!(name, "file_read" | "file_search") {
		scope.require_current(&area, run).await?;
	}
	if let Some(pod) = pod.as_ref()
		&& aidash_domain::tool::concurrency::pod_file_call(name, &input)
	{
		let prepared = prepare_pod_file(scope, run, &area, name, &input).await?;
		return match prepared {
			Prepared::Replay(value) => {
				Ok(Invocation::Complete(envelope(scope, &area, key, value)?))
			}
			Prepared::Call(operation) => Ok(Invocation::Pod(Box::new(DeferredPodFile {
				operation,
				access: PodFileAccess {
					area,
					pod: pod.clone(),
					policy_revision: scope.policy_revision(),
				},
				input,
			}))),
		};
	}
	let result = match (name, pod.as_ref()) {
		("file_remove", _) => pod_save::remove(scope, run, &mut area, parse(input)?).await?,
		("process_list", Some(pod)) => {
			super::pod::processes::list(pod, &run.id.to_string()).await?
		}
		("process_log", Some(pod)) => {
			super::pod::processes::log(pod, &run.id.to_string(), parse(input)?).await?
		}
		("process_stop", Some(pod)) => {
			super::pod::processes::stop(pod, &run.id.to_string(), parse(input)?).await?
		}
		_ => legacy(scope, run, name, &mut area, legacy_input(name, input), key).await?,
	};
	Ok(Invocation::Complete(envelope(scope, &area, key, result)?))
}
/// Tools that exist only with Execution Pods (#745).
const POD_TOOLS: &[&str] = &[
	"file_save",
	"file_remove",
	"file_copy",
	"process_list",
	"process_log",
	"process_stop",
];
/// The fields a caller left null are absent.
fn without_nulls(mut input: Value) -> Value {
	if let Some(object) = input.as_object_mut() {
		object.retain(|_, value| !value.is_null());
	}
	input
}
/// A `file_read` or `file_search` call in the 1.0.1 schema, as the Working Area
/// tools read it: the nullable fields that schema added are absent when null,
/// and an omitted `representation` reads text.
fn legacy_input(name: &str, input: Value) -> Value {
	if !matches!(name, "file_read" | "file_search") {
		return input;
	}
	let mut input = without_nulls(input);
	if name == "file_read"
		&& let Some(object) = input.as_object_mut()
	{
		object
			.entry("representation")
			.or_insert_with(|| json!("text"));
	}
	input
}
fn parse<T: serde::de::DeserializeOwned>(input: Value) -> Result<T> {
	serde_json::from_value(without_nulls(input)).map_err(|e| Error::Invalid(e.to_string()))
}
fn scratch_read(input: Value) -> Result<ScratchRead> {
	let mut input = without_nulls(input);
	if let Some(object) = input.as_object_mut() {
		match object
			.remove("representation")
			.as_ref()
			.and_then(Value::as_str)
		{
			None | Some("text") => {}
			Some(_) => {
				return Err(Error::Invalid(
					"a Scratch Space file is read as text; save it to the Working Area for other representations".into(),
				));
			}
		}
	}
	parse(input)
}
pub(crate) enum Prepared<T> {
	Replay(Value),
	Call(T),
}

pub enum Invocation {
	Complete(Envelope),
	Pod(Box<DeferredPodFile>),
}

enum PodFile {
	Read(ScratchRead, usize),
	Search(ScratchSearch, crate::ports::capabilities::files::Limits),
	Patch {
		patch: String,
		key: Uuid,
		digest: String,
		claim: Uuid,
		apply: bool,
	},
	Save(pod_save::Save),
	Copy(pod_save::Copy),
}

struct PodFileAccess {
	pod: PodContext,
	area: Area,
	policy_revision: i64,
}

impl PodFileAccess {
	async fn recheck(&self, scope: &mut dyn FileScopePort, run: &RunMetadata) -> Result<Area> {
		if scope.policy_revision() != self.policy_revision {
			return Err(Error::Forbidden);
		}
		let area = scope.for_run(run).await?;
		let pod = scope.recheck_pod(run).await?.ok_or(Error::Forbidden)?;
		if pod.settings != self.pod.settings
			|| pod.credential != self.pod.credential
			|| area.id != self.area.id
			|| area.generation != self.area.generation
			|| area.epoch != self.area.epoch
		{
			return Err(Error::Forbidden);
		}
		Ok(area)
	}
}

pub struct DeferredPodFile {
	operation: PodFile,
	access: PodFileAccess,
	input: Value,
}

enum PodFileResult<U> {
	Read(Value),
	Patch {
		value: Value,
		key: Uuid,
		digest: String,
		claim: Uuid,
	},
	Save(Box<pod_save::Downloaded<U>>),
	Copy(pod_save::Copy, Value),
}

pub struct CompletedPodFile<U> {
	result: PodFileResult<U>,
	access: PodFileAccess,
	input: Value,
}

async fn prepare_pod_file(
	scope: &mut dyn FileScopePort,
	run: &RunMetadata,
	area: &Area,
	name: &str,
	input: &Value,
) -> Result<Prepared<PodFile>> {
	Ok(Prepared::Call(match name {
		"file_read" => {
			scope.authorize(area, "file.read").await?;
			PodFile::Read(scratch_read(input.clone())?, scope.limits()?.read_bytes)
		}
		"file_search" => {
			scope.authorize(area, "file.read").await?;
			PodFile::Search(parse(input.clone())?, scope.limits()?)
		}
		"apply_patch" => {
			let key: Uuid = serde_json::from_value(input["idempotency_key"].clone())
				.map_err(|_| Error::Invalid("idempotency_key must be a UUID".into()))?;
			let patch = input["patch"]
				.as_str()
				.ok_or_else(|| Error::Invalid("patch must be a string".into()))?
				.to_owned();
			let digest =
				aidash_domain::registry::rules::digest(&json!(["scratch_patch", run.id, patch]));
			require_scratch_write(scope, run, area).await?;
			let (claim, apply) = match scope.cached(key, &digest).await? {
				Some(value) if value.is_string() => (serde_json::from_value(value)?, false),
				Some(value) => return Ok(Prepared::Replay(value)),
				None => {
					aidash_domain::capabilities::patch::parse(&patch)?;
					// Under the request's advisory lock, reserve the principal's key.
					// A string UUID is the pending claim; the completed result is an object.
					let claim = Uuid::new_v4();
					scope.cache(key, &digest, &json!(claim)).await?;
					(claim, true)
				}
			};
			PodFile::Patch {
				patch,
				key,
				digest,
				claim,
				apply,
			}
		}
		"file_save" => {
			match pod_save::Save::prepare(scope, run, area, parse(input.clone())?).await? {
				Prepared::Replay(value) => return Ok(Prepared::Replay(value)),
				Prepared::Call(save) => PodFile::Save(save),
			}
		}
		"file_copy" => {
			match pod_save::Copy::prepare(scope, run, area, parse(input.clone())?).await? {
				Prepared::Replay(value) => return Ok(Prepared::Replay(value)),
				Prepared::Call(copy) => PodFile::Copy(copy),
			}
		}
		_ => unreachable!("Scratch Space file tool"),
	}))
}

async fn require_scratch_write(
	scope: &mut dyn FileScopePort,
	run: &RunMetadata,
	area: &Area,
) -> Result<()> {
	scope.authorize(area, "file.write").await?;
	if scope.current_run(area).await? != Some(run.id) {
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	Ok(())
}

impl DeferredPodFile {
	/// Run the Pod calls with the authority and Working Area transactions released.
	pub async fn execute<S: ScratchStager>(
		self,
		run: &RunMetadata,
		stage: &mut S,
	) -> Result<CompletedPodFile<<S::Upload as ScratchUpload>::Sealed>> {
		let Self {
			operation,
			access,
			input,
		} = self;
		let pod = &access.pod;
		let result = match operation {
			PodFile::Read(read, bytes) => {
				PodFileResult::Read(pod_files::read(pod, read, bytes).await?)
			}
			PodFile::Search(search, limits) => PodFileResult::Read(
				tokio::time::timeout(
					std::time::Duration::from_secs(limits.search_seconds),
					pod_files::search(pod, search, limits.search_matches, limits.search_bytes),
				)
				.await
				.map_err(|_| Error::Invalid("SEARCH_TIME_LIMIT".into()))??,
			),
			PodFile::Patch {
				patch,
				key,
				digest,
				claim,
				apply,
			} => PodFileResult::Patch {
				value: pod_files::patch(pod, &run.id.to_string(), patch, claim, apply).await?,
				key,
				digest,
				claim,
			},
			PodFile::Save(save) => {
				PodFileResult::Save(Box::new(save.download(stage, &access.area, pod).await?))
			}
			PodFile::Copy(mut copy) => {
				let value = copy.execute(run, pod).await?;
				PodFileResult::Copy(copy, value)
			}
		};
		Ok(CompletedPodFile {
			result,
			access,
			input,
		})
	}
}

impl<U: Send> CompletedPodFile<U> {
	/// Recheck the Run's authority before disclosing bytes or committing results.
	pub async fn settle(
		self,
		scope: &mut dyn ScratchPublisher<Upload = U>,
		run: &RunMetadata,
		name: &str,
		key: &str,
	) -> Result<Envelope> {
		admit(scope, run, name, self.input).await?;
		let mut area = self.access.recheck(scope, run).await?;
		let value = match self.result {
			PodFileResult::Read(value) => {
				scope.require_current(&area, run).await?;
				scope.authorize(&area, "file.read").await?;
				value
			}
			PodFileResult::Patch {
				value,
				key,
				digest,
				claim,
			} => {
				require_scratch_write(scope, run, &area).await?;
				match scope.cached(key, &digest).await? {
					Some(previous) if previous == json!(claim) => {
						scope.complete_patch(key, &digest, claim, &value).await?;
						value
					}
					Some(previous) if previous.is_object() => previous,
					_ => return Err(Error::Conflict("PATCH_CLAIM_CHANGED".into())),
				}
			}
			PodFileResult::Save(save) => {
				available(&area.state)?;
				save.publish(scope, run, &mut area, self.access.area.revision)
					.await?
			}
			PodFileResult::Copy(copy, value) => {
				if area.revision != self.access.area.revision {
					return Err(Error::Conflict(
						aidash_domain::capabilities::errors::area_revision_changed(area.revision),
					));
				}
				copy.settle(scope, run, &area, value).await?
			}
		};
		envelope(scope, &area, key, value)
	}
}
/// The tools of the Working Area and the Runner's per-command path.
async fn legacy(
	scope: &mut dyn FileScopePort,
	run: &RunMetadata,
	name: &str,
	area: &mut Area,
	input: Value,
	key: &str,
) -> Result<Value> {
	Ok(match name {
		"code_interpreter" | "shell" => {
			let kind = if name == "shell" {
				super::pod::SHELL
			} else {
				super::pod::PYTHON
			};
			// Commands run only in the Execution Pod of the Run's Scratch Space (#745).
			let pod = scope.pod(run).await?.ok_or_else(|| {
				Error::Conflict("RUNTIME_UNAVAILABLE: no isolated runner configured".into())
			})?;
			let action = Action::PodCommand {
				kind,
				scratch_space_id: pod.scratch_space_id().to_owned(),
			};
			scope.execute(action, area, input, key).await?
		}
		"outbound_get" => scope.execute(Action::Outbound, area, input, key).await?,
		"file_share" => scope.execute(Action::Share, area, input, key).await?,
		"skill_list" | "skill_load" | "skill_read" => {
			scope.execute(Action::Skill(name), area, input, key).await?
		}
		"apply_patch" => scope.execute(Action::Patch, area, input, key).await?,
		"shell_poll" | "shell_cancel" | "python_poll" | "python_cancel" => {
			scope
				.execute(
					Action::Control {
						kind: if name.starts_with("python_") {
							"code_interpreter"
						} else {
							"shell"
						},
						cancel: matches!(name, "shell_cancel" | "python_cancel"),
					},
					area,
					input,
					key,
				)
				.await?
		}
		"file_read" => {
			let selection: FileRead =
				serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?;
			match (selection.thread_id, selection.area_id) {
				(Some(thread), Some(target)) => {
					*area = scope.subnode_area(run, thread, target).await?;
					// A member's Reference set is read only inside the member's own Run,
					// whose Agent version binds it; a copy the member made is readable (ADR 0114).
					if files(area)?
						.iter()
						.any(|file| file.file_id == selection.file_id && file.is_reference_file())
					{
						return Err(Error::NotFound("file unavailable".into()));
					}
				}
				(None, None) => {}
				_ => {
					return Err(Error::Invalid(
						"thread_id and area_id must be supplied together".into(),
					));
				}
			}
			if matches!(selection.representation, Representation::ModelInput) {
				let file = files(area)?
					.into_iter()
					.find(|file| file.file_id == selection.file_id)
					.ok_or_else(|| Error::NotFound("file unavailable".into()))?;
				ContentPart::modality_for_media_type(&file.media_type)?;
				let agent = scope
					.effective(&EntityRef {
						id: run.agent_id.clone(),
						version: run.agent_version.clone(),
					})
					.await?;
				let agent: AgentConfig = serde_json::from_value(agent.config)?;
				let entry = scope.entry(&agent.model, "registry.read").await?;
				let effective = scope
					.effective(&EntityRef {
						id: entry.id,
						version: entry.version,
					})
					.await?;
				let model: ModelConfig = ModelConfig::from_stored(effective.config)?;
				model.require_media_types([file.media_type.as_str()])?;
			}
			read(scope, area, selection).await?
		}
		"file_search" => tokio::time::timeout(
			std::time::Duration::from_secs(scope.limits()?.search_seconds),
			search(
				scope,
				area,
				serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?,
			),
		)
		.await
		.map_err(|_| Error::Invalid("SEARCH_TIME_LIMIT".into()))??,
		_ => return Err(Error::Forbidden),
	})
}
/// Shape a tool result as the Envelope Agents read.
pub fn envelope(
	scope: &mut dyn FileScopePort,
	area: &Area,
	key: &str,
	mut result: Value,
) -> Result<Envelope> {
	let operation_id = result
		.get_mut("operation_id")
		.map(Value::take)
		.and_then(|v| v.as_str().map(str::to_owned))
		.unwrap_or_else(|| key.to_owned());
	let status = result
		.get_mut("status")
		.map(Value::take)
		.and_then(|v| v.as_str().map(str::to_owned))
		.unwrap_or_else(|| "completed".into());
	let generation = result["generation"].as_i64().unwrap_or(area.generation);
	let revision = result["revision"].as_i64().unwrap_or(area.revision);
	if let Some(object) = result.as_object_mut() {
		for field in [
			"operation_id",
			"status",
			"policy_revision",
			"area_id",
			"generation",
			"revision",
		] {
			object.remove(field);
		}
		if let Some(error) = object.get_mut("error") {
			*error = serde_json::to_value(CapabilityError::stored(error))?;
		}
	}
	Ok(Envelope {
		operation_id,
		status,
		policy_revision: scope.policy_revision(),
		area_id: area.id,
		generation,
		revision,
		result,
	})
}
pub async fn read(scope: &mut dyn FileScopePort, area: &Area, input: FileRead) -> Result<Value> {
	let file = files(area)?
		.into_iter()
		.find(|f| f.file_id == input.file_id)
		.ok_or_else(|| Error::NotFound("file unavailable".into()))?;
	require_usable(scope, &file).await?;
	if input
		.expected_digest
		.as_ref()
		.is_some_and(|digest| digest != &file.digest)
	{
		return Err(Error::Conflict("FILE_CHANGED".into()));
	}
	if matches!(input.representation, Representation::ModelInput) {
		if input.expected_digest.as_deref() != Some(file.digest.as_str())
			|| input.offset.is_some()
			|| input.max_bytes.is_some()
		{
			return Err(Error::Invalid(
				"model input requires an exact digest and no read range".into(),
			));
		}
		if file.size > 8 * 1024 * 1024 {
			return Err(Error::Invalid(
				"model media input exceeds byte limit".into(),
			));
		}
		ContentPart::modality_for_media_type(&file.media_type)?;
		return Ok(json!({"metadata":file,"digest":file.digest,"truncated":false}));
	}
	if matches!(input.representation, Representation::Metadata) {
		return Ok(json!({"metadata":file,"digest":file.digest,"truncated":false}));
	}
	let offset = input.offset.unwrap_or(0);
	let limit = input.max_bytes.unwrap_or(scope.limits()?.read_bytes);
	if limit == 0 || limit > scope.limits()?.read_bytes || offset as u64 > file.size {
		return Err(Error::Invalid("INVALID_READ_RANGE".into()));
	}
	let mut object = tokio::time::timeout(
		std::time::Duration::from_secs(scope.limits()?.search_seconds),
		scope.open(&file),
	)
	.await
	.map_err(|_| Error::Invalid("READ_TIME_LIMIT".into()))??;
	let bytes = object.read_range(offset as u64, limit as u64).await?;
	let text = match std::str::from_utf8(&bytes) {
		Ok(text) => text,
		Err(error) if error.error_len().is_none() => {
			std::str::from_utf8(&bytes[..error.valid_up_to()]).map_err(|_| Error::Forbidden)?
		}
		Err(_) => {
			return Err(Error::Invalid(
				"REPRESENTATION_UNAVAILABLE_OR_INVALID_UTF8_OFFSET".into(),
			));
		}
	};
	let end = offset + text.len();
	if end == offset && (offset as u64) < file.size {
		return Err(Error::Invalid(
			"READ_BUDGET: max_bytes cannot contain the next UTF-8 character".into(),
		));
	}
	Ok(
		json!({"content":text,"metadata":file,"digest":file.digest,"encoding":"utf8","next_offset":((end as u64)<file.size).then_some(end),"truncated":(end as u64)<file.size}),
	)
}
pub async fn search(
	scope: &mut dyn FileScopePort,
	area: &Area,
	input: FileSearch,
) -> Result<Value> {
	let limit = input.limit.unwrap_or(scope.limits()?.search_matches);
	// An empty query lists paths; only path mode has a meaning for it.
	if (input.query.is_empty() && !matches!(input.mode, SearchMode::Path))
		|| input.query.len() > 1024
		|| !(1..=scope.limits()?.search_matches).contains(&limit)
	{
		return Err(Error::Invalid("INVALID_SEARCH_BUDGET".into()));
	}
	if let Some(path) = &input.path {
		aidash_domain::registry::rules::validate_path(path)?;
	}
	let pattern = match input.mode {
		SearchMode::Regex => Some(
			regex::RegexBuilder::new(&input.query)
				.size_limit(65536)
				.build()
				.map_err(|_| Error::Invalid("INVALID_REGEX".into()))?,
		),
		_ => None,
	};
	let hash = aidash_domain::registry::rules::digest(&json!([
		input.query,
		input.mode,
		input.scope,
		input.path
	]));
	let mut cursor = match &input.cursor {
		Some(encoded) if encoded.len() <= 2048 => serde_json::from_slice::<Cursor>(
			&URL_SAFE_NO_PAD
				.decode(encoded)
				.map_err(|_| Error::Invalid("INVALID_CURSOR".into()))?,
		)
		.map_err(|_| Error::Invalid("INVALID_CURSOR".into()))?,
		Some(_) => return Err(Error::Invalid("INVALID_CURSOR".into())),
		None => Cursor {
			area: area.id,
			generation: area.generation,
			revision: area.revision,
			query: hash.clone(),
			file: 0,
			line: 0,
		},
	};
	if cursor.area != area.id
		|| cursor.generation != area.generation
		|| cursor.revision != area.revision
		|| cursor.query != hash
	{
		return Err(Error::Conflict("STALE_CURSOR".into()));
	}
	let files = files(area)?;
	// A Reference file revoked after the Area was given it drops out of the search
	// and the rest stay searchable. One query covers the whole Reference set.
	let revoked = if input.scope == FileScope::References {
		let ids = files.iter().filter_map(reference_of).collect::<Vec<_>>();
		if ids.is_empty() {
			BTreeSet::new()
		} else {
			scope.unusable_references(&ids).await?
		}
	} else {
		BTreeSet::new()
	};
	let started = std::time::Instant::now();
	// Stop early enough to return what was found before the caller's timeout,
	// with a cursor to continue. Always make some progress first.
	let budget = std::time::Duration::from_secs(scope.limits()?.search_seconds).mul_f32(0.8);
	let mut scanned = false;
	let mut matches = vec![];
	let mut unavailable: Vec<Value> = vec![];
	'files: while cursor.file < files.len() {
		if scanned && started.elapsed() > budget {
			break;
		}
		let file = &files[cursor.file];
		if serde_json::to_value(file.scope)? != serde_json::to_value(input.scope)?
			|| input.path.as_ref().is_some_and(|path| {
				file.path != *path && !file.path.starts_with(&format!("{path}/"))
			}) || reference_of(file).is_some_and(|id| revoked.contains(&id))
		{
			cursor.file += 1;
			cursor.line = 0;
			continue;
		}
		let mut reader: Box<dyn VerifiedFile> = if matches!(input.mode, SearchMode::Path) {
			Box::new(MemoryReader {
				bytes: file.path.as_bytes().to_vec(),
				offset: 0,
			})
		} else {
			scope.open(file).await?
		};
		let mut exhausted = true;
		let mut index = 0;
		loop {
			let line = match bounded_line(&mut *reader).await {
				Ok(Some(line)) => line,
				Ok(None) => break,
				// A file with a very long line, such as minified JSON, must not
				// stop a search of everything else.
				Err(Error::Invalid(message))
					if message.starts_with("SEARCH_LINE_LIMIT")
						&& input.path.as_deref() != Some(&file.path) =>
				{
					// A full page must not lose what it found: stop here with a cursor, and
					// the next page, which starts empty, reports this file and moves on.
					if matches.len() + unavailable.len() >= limit {
						cursor.line = index;
						exhausted = false;
						break;
					}
					unavailable.push(json!({"file_id":file.file_id,"error":"SEARCH_LINE_LIMIT"}));
					break;
				}
				Err(error) => return Err(error),
			};
			let index = {
				let current = index;
				index += 1;
				current
			};
			if index < cursor.line {
				continue;
			}
			let Ok(line) = std::str::from_utf8(&line) else {
				if input.path.as_deref() == Some(&file.path) {
					return Err(Error::Invalid(
						"REPRESENTATION_UNAVAILABLE: invalid UTF-8".into(),
					));
				}
				let item = json!({"file_id":file.file_id,"error":"REPRESENTATION_UNAVAILABLE"});
				let bytes = matches
					.iter()
					.chain(&unavailable)
					.map(|v: &Value| v.to_string().len())
					.sum::<usize>() + item.to_string().len();
				if matches.len() + unavailable.len() >= limit
					|| bytes > scope.limits()?.search_bytes.saturating_sub(2768)
				{
					cursor.line = index;
					exhausted = false;
					break;
				}
				unavailable.push(item);
				break;
			};
			if scanned && index % 64 == 0 && started.elapsed() > budget {
				cursor.line = index;
				break 'files;
			}
			scanned = true;
			let found = pattern
				.as_ref()
				.map_or_else(|| line.contains(&input.query), |p| p.is_match(line));
			if found {
				let mut end = line.len().min(512);
				while !line.is_char_boundary(end) {
					end -= 1;
				}
				let item = json!({"file_id":file.file_id,"path":file.path,"digest":file.digest,"location":{"line":index+1,"source":file.provenance},"snippet":&line[..end]});
				let byte_count = matches
					.iter()
					.chain(&unavailable)
					.map(|v: &Value| v.to_string().len())
					.sum::<usize>() + item.to_string().len();
				if matches.len() + unavailable.len() >= limit
					|| byte_count > scope.limits()?.search_bytes.saturating_sub(2768)
				{
					if matches.is_empty() && unavailable.is_empty() {
						return Err(Error::Invalid(
							"SEARCH_RESULT_LIMIT: one match exceeds the configured page budget"
								.into(),
						));
					}
					cursor.line = index;
					exhausted = false;
					break;
				}
				matches.push(item);
			}
			cursor.line = index + 1;
		}
		if !exhausted {
			break;
		}
		cursor.file += 1;
		cursor.line = 0;
	}
	let next = (cursor.file < files.len()).then(|| {
		URL_SAFE_NO_PAD.encode(serde_json::to_vec(&cursor).expect("cursor serialization"))
	});
	Ok(
		json!({"matches":matches,"unavailable":unavailable,"truncated":next.is_some(),"next_cursor":next}),
	)
}
async fn bounded_line(reader: &mut dyn VerifiedFile) -> Result<Option<Vec<u8>>> {
	let mut output = vec![];
	loop {
		let bytes = reader.fill_buf().await?;
		if bytes.is_empty() {
			return Ok((!output.is_empty()).then_some(output));
		}
		let end = bytes.iter().position(|b| *b == b'\n');
		let count = end.map_or(bytes.len(), |n| n + 1);
		if output.len() + count > 65536 {
			return Err(Error::Invalid(
				"SEARCH_LINE_LIMIT: select a smaller text representation".into(),
			));
		}
		output.extend_from_slice(&bytes[..count]);
		reader.consume(count);
		if end.is_some() {
			output.pop();
			return Ok(Some(output));
		}
	}
}
struct MemoryReader {
	bytes: Vec<u8>,
	offset: usize,
}
#[async_trait::async_trait]
impl VerifiedFile for MemoryReader {
	async fn read_range(&mut self, offset: u64, limit: u64) -> Result<Vec<u8>> {
		let offset =
			usize::try_from(offset).map_err(|_| Error::Invalid("INVALID_READ_RANGE".into()))?;
		if offset > self.bytes.len() {
			return Err(Error::Invalid("INVALID_READ_RANGE".into()));
		}
		Ok(
			self.bytes[offset..self.bytes.len().min(offset.saturating_add(limit as usize))]
				.to_vec(),
		)
	}
	async fn fill_buf(&mut self) -> Result<&[u8]> {
		Ok(&self.bytes[self.offset..])
	}
	fn consume(&mut self, count: usize) {
		self.offset = self.bytes.len().min(self.offset + count);
	}
}
pub async fn materialize(
	scope: &mut dyn FileScopePort,
	run: &RunMetadata,
	input: Materialize,
) -> Result<Value> {
	if !settings(scope, run).await?.core_capabilities.files {
		return Err(Error::Forbidden);
	}
	let mut area = scope.for_run(run).await?;
	available(&area.state)?;
	scope.authorize(&area, "file.write").await?;
	let digest = aidash_domain::registry::rules::digest(&json!(["materialize", run.id, input]));
	if let Some(cached) = scope.cached(input.idempotency_key, &digest).await? {
		return Ok(cached);
	}
	if scope.current_run(&area).await? != Some(run.id) {
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	if input.expected_revision != area.revision {
		return Err(Error::Conflict(
			aidash_domain::capabilities::errors::area_revision_changed(area.revision),
		));
	}
	aidash_domain::registry::rules::validate_path(&input.path)?;
	let mut entries = files(&area)?;
	if aidash_domain::capabilities::operations::counted_files(&entries).count() >= 4096
		|| entries.iter().any(|f| f.path == input.path)
	{
		return Err(Error::Conflict("FILE_EXISTS_OR_LIMIT".into()));
	}
	if let MaterializeSource::File {
		file_id,
		expected_digest,
	} = &input.source
	{
		let original = match entries.iter().find(|f| f.file_id == *file_id).cloned() {
			Some(file) => file,
			None => output_file(scope, &area, *file_id).await?,
		};
		require_usable(scope, &original).await?;
		if original.digest != *expected_digest {
			return Err(Error::Conflict("FILE_CHANGED".into()));
		}
		if aidash_domain::capabilities::operations::counted_files(&entries)
			.map(|f| f.size)
			.sum::<u64>()
			+ original.size
			+ scope.retained_bytes(&area).await?
			> scope.limits()?.working_bytes
		{
			return Err(Error::Conflict("WORKING_QUOTA".into()));
		}
		scope.begin_pending(area.id, original.size).await?;
		let mut offset = 0;
		while offset < original.size {
			let block = scope.read_chunk(&original, offset).await?;
			if block.is_empty() {
				return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
			}
			offset += block.len() as u64;
			scope.write_pending(&block).await?;
		}
		let (file_id, digest_value) = scope.finish_pending(expected_digest).await?;
		// A copy of a Reference file lives only while that file may still be used.
		if original.is_reference_file() {
			let constraint = json!({"kind":"reference","id":original.provenance["id"]});
			let constraints = area.constraints.as_array_mut().ok_or(Error::Forbidden)?;
			if !constraints.contains(&constraint) {
				constraints.push(constraint);
			}
		}
		let file = FileEntry {
			file_id,
			path: input.path,
			digest: digest_value,
			scope: FileScope::Working,
			..original
		};
		entries.push(file.clone());
		area.manifest = json!(entries);
		publish(scope, &mut area).await?;
		let result = json!({"area_id":area.id,"revision":area.revision,"file":file});
		scope.cache(input.idempotency_key, &digest, &result).await?;
		return Ok(result);
	}
	let (text, provenance, file_scope) = match input.source {
		MaterializeSource::File { .. } => unreachable!("file copies are streamed above"),
		MaterializeSource::Message { message_id } => {
			let message = scope.message(area.thread_id, message_id).await?;
			(
				message["content"]
					.as_str()
					.ok_or(Error::Forbidden)?
					.to_owned(),
				json!({"kind":"message","id":message_id}),
				FileScope::Working,
			)
		}
		MaterializeSource::ReferenceText { index } => {
			let entry = scope
				.entry(
					&EntityRef {
						id: run.agent_id.clone(),
						version: run.agent_version.clone(),
					},
					"agent.execute",
				)
				.await?;
			let references = scope.documents(&entry).await?;
			let document = references
				.get(index)
				.ok_or_else(|| Error::NotFound("reference unavailable".into()))?;
			(
				document["text"]
					.as_str()
					.ok_or(Error::Forbidden)?
					.to_owned(),
				json!({"kind":"reference_text","agent":{"id":run.agent_id,"version":run.agent_version},"index":index,"name":document["name"]}),
				FileScope::References,
			)
		}
	};
	let size = aidash_domain::capabilities::operations::counted_files(&entries)
		.map(|f| f.size)
		.sum::<u64>()
		+ text.len() as u64
		+ scope.retained_bytes(&area).await?;
	if size > scope.limits()?.working_bytes {
		return Err(Error::Conflict("WORKING_QUOTA".into()));
	}
	let file = scope
		.text_file(area.id, input.path, &text, file_scope, provenance.clone())
		.await?;
	entries.push(file.clone());
	area.manifest = json!(entries);
	let constraints = area.constraints.as_array_mut().ok_or(Error::Forbidden)?;
	if matches!(
		provenance["kind"].as_str(),
		Some("message" | "reference_text")
	) && !constraints.contains(&provenance)
	{
		constraints.push(provenance);
	}
	publish(scope, &mut area).await?;
	let published = files(&area)?
		.into_iter()
		.find(|entry| entry.file_id == file.file_id)
		.ok_or_else(|| Error::Conflict("published file unavailable".into()))?;
	let result = json!({"area_id":area.id,"revision":area.revision,"file":published});
	scope.cache(input.idempotency_key, &digest, &result).await?;
	Ok(result)
}

pub enum AttachmentCopy {
	Complete(Value),
	Pod(Box<PodAttachmentCopy>),
}

pub struct PodAttachmentCopy {
	access: PodFileAccess,
	key: Uuid,
	request_digest: String,
	path: String,
	pub message_id: Uuid,
	attachment_id: Uuid,
	digest: String,
	media_type: String,
}

impl PodAttachmentCopy {
	pub async fn execute(&self, run: &RunMetadata, bytes: &[u8]) -> Result<Value> {
		let size = pod_files::write(
			&self.access.pod,
			&run.id.to_string(),
			&self.path,
			bytes,
			None,
		)
		.await?;
		Ok(json!({"path":self.path,"size":size,"digest": self.digest,
			"media_type":self.media_type,"location":"scratch",
			"source":{"message_id":self.message_id,"attachment_id":self.attachment_id}}))
	}

	pub async fn settle(
		self,
		scope: &mut dyn FileScopePort,
		run: &RunMetadata,
		result: Value,
	) -> Result<Value> {
		if !settings(scope, run).await?.core_capabilities.files {
			return Err(Error::Forbidden);
		}
		let area = self.access.recheck(scope, run).await?;
		require_scratch_write(scope, run, &area).await?;
		if let Some(cached) = scope.cached(self.key, &self.request_digest).await? {
			return Ok(cached);
		}
		scope.cache(self.key, &self.request_digest, &result).await?;
		Ok(result)
	}
}

/// Copy verified Thread Attachment bytes through the Working Area publication path.
#[allow(clippy::too_many_arguments)]
pub async fn copy_thread_attachment(
	scope: &mut dyn FileScopePort,
	run: &RunMetadata,
	key: Uuid,
	expected_revision: i64,
	path: String,
	message_id: Uuid,
	attachment_id: Uuid,
	media_type: String,
	expected_digest: String,
	bytes: &[u8],
) -> Result<AttachmentCopy> {
	if !settings(scope, run).await?.core_capabilities.files {
		return Err(Error::Forbidden);
	}
	// Where commands run in an Execution Pod, the attachment lands in the
	// Scratch Space they read (#745).
	if let Some(pod) = scope.pod(run).await? {
		let area = scope.for_run(run).await?;
		scope.authorize(&area, "file.write").await?;
		let operation_digest = aidash_domain::registry::rules::digest(&json!([
			"copy_thread_attachment_scratch",
			run.id,
			path,
			message_id,
			attachment_id,
			expected_digest
		]));
		if let Some(cached) = scope.cached(key, &operation_digest).await? {
			return Ok(AttachmentCopy::Complete(cached));
		}
		if scope.current_run(&area).await? != Some(run.id) {
			return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
		}
		return Ok(AttachmentCopy::Pod(Box::new(PodAttachmentCopy {
			access: PodFileAccess {
				pod,
				area,
				policy_revision: scope.policy_revision(),
			},
			key,
			request_digest: operation_digest,
			path,
			message_id,
			attachment_id,
			digest: expected_digest,
			media_type,
		})));
	}
	let mut area = scope.for_run(run).await?;
	available(&area.state)?;
	scope.authorize(&area, "file.write").await?;
	let operation_digest = aidash_domain::registry::rules::digest(&json!([
		"copy_thread_attachment",
		run.id,
		expected_revision,
		path,
		message_id,
		attachment_id,
		expected_digest
	]));
	if let Some(cached) = scope.cached(key, &operation_digest).await? {
		return Ok(AttachmentCopy::Complete(cached));
	}
	if scope.current_run(&area).await? != Some(run.id) {
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	if area.revision != expected_revision {
		return Err(Error::Conflict("AREA_REVISION_CHANGED".into()));
	}
	aidash_domain::registry::rules::validate_path(&path)?;
	let mut entries = files(&area)?;
	if aidash_domain::capabilities::operations::counted_files(&entries).count() >= 4096
		|| entries.iter().any(|entry| entry.path == path)
	{
		return Err(Error::Conflict("FILE_EXISTS_OR_LIMIT".into()));
	}
	let size = bytes.len() as u64;
	if aidash_domain::capabilities::operations::counted_files(&entries)
		.map(|f| f.size)
		.sum::<u64>()
		+ size + scope.retained_bytes(&area).await?
		> scope.limits()?.working_bytes
	{
		return Err(Error::Conflict("WORKING_QUOTA".into()));
	}
	scope.begin_pending(area.id, size).await?;
	scope.write_pending(bytes).await?;
	let (file_id, digest) = scope.finish_pending(&expected_digest).await?;
	let file = FileEntry {
		file_id,
		path,
		digest,
		size,
		media_type,
		scope: FileScope::Working,
		provenance: json!({"kind":"thread_attachment", "thread_id":area.thread_id, "message_id":message_id, "attachment_id":attachment_id}),
	};
	entries.push(file.clone());
	area.manifest = json!(entries);
	// A local Area stays usable only while its Thread message may still be read. A receiving
	// Area (ADR 0139) gets no such source: the message is the Home's, which this Node cannot
	// read, so every later check of the Area would refuse. The Home checked the pull, the file
	// keeps its provenance, the Area sends files to that Home only, and a copy into the Scratch
	// Space above carries no source either.
	if !area.is_remote_home() {
		let source = json!({"kind":"message", "id":message_id});
		let constraints = area.constraints.as_array_mut().ok_or(Error::Forbidden)?;
		if !constraints.contains(&source) {
			constraints.push(source);
		}
	}
	publish(scope, &mut area).await?;
	let result = json!({"area_id":area.id, "revision":area.revision, "file":file});
	scope.cache(key, &operation_digest, &result).await?;
	Ok(AttachmentCopy::Complete(result))
}

/// Resolve a Run's current Working Area paths into verified immutable snapshots.
/// A committed message receipt is checked before reading mutable files; the
/// owner must cache the receipt in the same transaction as the message.
pub async fn snapshot_message_files_once(
	scope: &mut dyn FileScopePort,
	run: &RunMetadata,
	key: Uuid,
	digest: &str,
	committed: bool,
	paths: &[String],
) -> Result<Option<Vec<(FileEntry, Vec<u8>)>>> {
	if scope.cached(key, digest).await?.is_some() || committed {
		return Ok(None);
	}
	snapshot_message_files(scope, run, paths).await.map(Some)
}

pub async fn snapshot_message_files(
	scope: &mut dyn FileScopePort,
	run: &RunMetadata,
	paths: &[String],
) -> Result<Vec<(FileEntry, Vec<u8>)>> {
	if paths.len() > 8 {
		return Err(Error::Invalid("at most 8 attachments".into()));
	}
	if !settings(scope, run).await?.core_capabilities.files {
		return Err(Error::Forbidden);
	}
	let area = scope.for_run(run).await?;
	if area.state != "running" {
		available(&area.state)?;
	}
	scope.require_current(&area, run).await?;
	scope.authorize(&area, "file.read").await?;
	let entries = files(&area)?;
	// A receiving Area's files go to its Home, another Tenant's Node: the rule of a share to
	// that Home holds (ADR 0139), so neither its Reference files nor copies of them as made
	// leave, and an Area holding private originals or received files sends none.
	let leaving = area.is_remote_home();
	if leaving {
		super::sharing::require_unbarred(&area)?;
	}
	let kept = if leaving {
		super::sharing::reference_digests(&entries)
	} else {
		BTreeSet::new()
	};
	let mut result = Vec::with_capacity(paths.len());
	let mut total = 0_u64;
	for path in paths {
		let file = entries
			.iter()
			.find(|file| {
				file.path == *path
					&& !file.is_reference_file()
					&& !kept.contains(file.digest.as_str())
			})
			.ok_or_else(|| Error::NotFound("file unavailable".into()))?
			.clone();
		total = total.saturating_add(file.size);
		if file.size > 8 * 1024 * 1024 || total > 8 * 1024 * 1024 {
			return Err(Error::Invalid("attachments exceed 8 MiB".into()));
		}
		let mut object = scope.open(&file).await?;
		let bytes = object.read_range(0, file.size).await?;
		if bytes.len() as u64 != file.size {
			return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
		}
		result.push((file, bytes));
	}
	Ok(result)
}
pub async fn publish(scope: &mut dyn FileScopePort, area: &mut Area) -> Result<()> {
	publish_with_known_media(scope, area, &BTreeSet::new()).await
}

pub(crate) async fn publish_with_known_media(
	scope: &mut dyn FileScopePort,
	area: &mut Area,
	known_media: &BTreeSet<Uuid>,
) -> Result<()> {
	// Commit the old working-object tombstones with the new manifest. Only
	// superseded working bytes are reclaimed, never outputs or recovery copies.
	let previous: Vec<FileEntry> = serde_json::from_value(scope.previous_manifest(area).await?)?;
	let previous_generation = scope.previous_generation(area).await?;
	let mut current = files(area)?;
	for file in &mut current {
		if matches!(file.scope, FileScope::Working | FileScope::Received)
			&& file.media_type == "application/octet-stream"
			&& !known_media.contains(&file.file_id)
			&& file.size > 0
			&& (previous_generation != area.generation
				|| !previous.iter().any(|old| old.file_id == file.file_id))
		{
			let mut reader = scope.open(file).await?;
			let prefix = reader.read_range(0, file.size.min(12)).await?;
			if let Some(media_type) = aidash_domain::provider::detect_image(&prefix) {
				file.media_type = media_type.into();
			}
		}
	}
	area.manifest = json!(current);
	for old in &previous {
		if matches!(old.scope, FileScope::Working)
			&& !current.iter().any(|file| file.file_id == old.file_id)
		{
			scope.supersede(area, old.file_id).await?;
		}
	}
	area.revision += 1;
	scope.persist_manifest(area).await?;
	let changes = if previous_generation != area.generation {
		let mut changes = aidash_domain::semantic::indexing::image_changes(&previous, &[]);
		changes.extend(aidash_domain::semantic::indexing::image_changes(
			&[],
			&current,
		));
		changes
	} else {
		aidash_domain::semantic::indexing::image_changes(&previous, &current)
	};
	// A receiving Area belongs to a Home whose Thread this Node does not hold: no Thread
	// image index or event stream here can take what it changes (ADR 0139).
	if area.is_remote_home() {
		return Ok(());
	}
	if !changes.is_empty() {
		scope.image_publication(area, &changes).await?;
	}
	scope
		.event(
			area.thread_id,
			"capability.files_changed",
			json!({"area_id":area.id,"revision":area.revision,"generation":area.generation}),
		)
		.await?;
	Ok(())
}
#[cfg(test)]
mod tests;
