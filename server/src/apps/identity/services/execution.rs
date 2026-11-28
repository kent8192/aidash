use super::{access::Access, catalog, identity::SubjectIdentity};
use crate::apps::identity::repositories::execution::Grant;
use crate::{
	Error, Result,
	apps::execution::serializers::runs::RunDetails,
	domain::*,
	federation::{Delegation, Discovery, Federation, Home},
	provider::ToolCall,
	registry::{AgentConfig, EntityRef, Search},
	store::Store,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

/// The Run's own authority was gone when its tool call had already had its effect. The harness
/// fills in the tool and leaves the invocation `STARTED`. A Node that did not answer is not
/// retried either ([`after_effect`]).
fn authority_gone_after_effect(error: Error) -> Error {
	match error {
		Error::Forbidden
		| Error::Unauthorized
		| Error::CredentialForbidden(_)
		| Error::CredentialUnauthorized(_) => {
			Error::AuthorityRefused(Box::new(aidash_domain::tool::AuthorityRefusal {
				tool: String::new(),
				check: aidash_domain::tool::RefusalCheck::RunAuthority,
				action: None,
				subject: None,
				call: None,
			}))
		}
		error => after_effect(error),
	}
}

/// What a core tool that the Run's own authority or the Run's state refuses answers: the
/// operation, blocked, with the error as an Agent reads it. Anything else is the call's failure.
fn blocked(key: &str, error: Error) -> Result<Value> {
	match error {
		error
		@ (Error::Invalid(_) | Error::Conflict(_) | Error::NotFound(_) | Error::Forbidden) => {
			let status = match &error {
				Error::Invalid(_) => 400,
				Error::Conflict(_) => 409,
				Error::NotFound(_) => 404,
				_ => 403,
			};
			Ok(
				json!({"operation_id":key,"status":"blocked","error":crate::capabilities::errors::CapabilityError::message(status,&error.to_string())}),
			)
		}
		error => Err(error),
	}
}

/// Give the Run's own lease back after a tool suspended it, on every path: an early refusal of
/// the tool's own authority can now reach the model as a Rejected Call, and the Run then goes on
/// with this lease. If the lease cannot be restored, the Run's authority is gone and it pauses,
/// whatever `result` says.
async fn restore_outer<T>(
	outer: &mut Access,
	store: &Store,
	run: &Run,
	result: Result<T>,
) -> Result<T> {
	refresh_access_for_run(outer, store, run)
		.await
		.map_err(authority_gone_after_effect)?;
	memory_run_check(outer, &run.metadata())
		.await
		.map_err(authority_gone_after_effect)?;
	result
}

async fn memory_run_check(access: &mut Access, run: &crate::domain::RunMetadata) -> Result<()> {
	let mark = access.call_mark();
	match access.run_invalidity(run).await? {
		None => Ok(()),
		Some(reason) => Err(access.memory_failure_since(mark, reason)),
	}
}

/// A Node that did not answer a recheck run after a tool took effect (#751):
/// running the tool again could repeat the effect, so the Run is not retried.
fn after_effect(error: Error) -> Error {
	match error {
		Error::NodeUnreachable { node, .. } => Error::NodeUnreachable {
			node,
			after_effect: true,
		},
		error => error,
	}
}

async fn grant(store: &Store, run: &crate::domain::RunMetadata) -> Result<Option<Grant>> {
	aidash_application::authorization::execution::grant(
		&crate::bootstrap::execution_grants(store),
		run,
	)
	.await
	.map(|grant| grant.map(Into::into))
	.map_err(Into::into)
}

pub(crate) async fn access_for_run(
	store: &Store,
	run: &crate::domain::RunMetadata,
	durable_audit: bool,
) -> Result<Option<Access>> {
	access_for_run_with_mode(store, run, durable_audit, false).await
}

/// A recovery proof must be read-only before binding the worker, whose initial
/// thread check would otherwise persist an authorization decision.
pub(crate) async fn proof_for_run(
	store: &Store,
	run: &crate::domain::RunMetadata,
) -> Result<Option<Access>> {
	access_for_run_with_mode(store, run, false, true).await
}
async fn access_for_run_with_mode(
	store: &Store,
	run: &crate::domain::RunMetadata,
	durable_audit: bool,
	proof: bool,
) -> Result<Option<Access>> {
	let Some(grant) = grant(store, run).await? else {
		return Ok(None);
	};
	let mut access = if proof {
		Access::begin_proof(store, &grant.identity()).await?
	} else {
		Access::begin(store, &grant.identity()).await?
	};
	aidash_application::authorization::execution::bind_worker(
		&mut crate::bootstrap::execution_grant_scope(&mut access),
		run,
		&grant.into(),
		durable_audit,
	)
	.await?;
	Ok(Some(access))
}

pub(crate) async fn inherit_run_authority(access: &mut Access, run: &Run) -> Result<()> {
	aidash_application::authorization::execution::inherit_run(
		&mut crate::bootstrap::execution_grant_scope(access),
		&run.metadata(),
	)
	.await
	.map_err(Into::into)
}

pub(in crate::apps::identity) async fn refresh_access_for_run(
	access: &mut Access,
	store: &Store,
	run: &Run,
) -> Result<()> {
	let mark = access.call_mark();
	let result = aidash_application::authorization::execution::refresh_worker(
		&mut crate::bootstrap::execution_grant_scope(access),
		store.deployment.as_str(),
		&run.metadata(),
	)
	.await
	.map_err(Into::into);
	result.map_err(|error| access.named_since(mark, error))
}

pub(crate) async fn inherit_task_origin(access: &mut Access, task: Uuid) -> Result<bool> {
	aidash_application::authorization::execution::inherit_task(
		&mut crate::bootstrap::execution_grant_scope(access),
		task,
	)
	.await
	.map_err(Into::into)
}

async fn admit(
	f: &Federation,
	access: &mut Access,
	task: Uuid,
	revision: Option<i64>,
	agent: &EntityRef,
	delegation: bool,
) -> Result<Task> {
	aidash_application::authorization::execution::admission::admit(
		&mut crate::bootstrap::execution_admission_scope(f, access),
		task,
		revision,
		agent,
		delegation,
	)
	.await
	.map_err(Into::into)
}

/// Whether an operation that acts for `run` may go on, as the operation sweep decides it: an
/// authority for the saved identity, with the subjects the operation saved (the Run's chain).
/// Tests drive the sweep's check through this seam; production uses `run_for_action` directly.
#[doc(hidden)]
pub async fn operation_may_act(
	store: &Store,
	identity: &SubjectIdentity,
	subjects: Vec<String>,
	run: Uuid,
) -> Result<()> {
	let mut access = Access::begin(store, identity).await?;
	access.subjects = subjects;
	let result = access.run_for_action(run).await.map(|_| ());
	access.finish(result).await
}

/// The Use Recheck alone, for tests: the first definition of `run`'s Binding (or its Agent) that
/// these subjects may no longer use. `local_only` is the resume's rule, which leaves out a Run of
/// another Home executing here.
#[doc(hidden)]
pub async fn use_recheck_blocks(
	store: &Store,
	identity: &SubjectIdentity,
	subjects: Vec<String>,
	run: &crate::domain::RunMetadata,
	local_only: bool,
) -> Result<Option<EntityRef>> {
	let mut access = Access::begin(store, identity).await?;
	access.subjects = subjects;
	let result = crate::apps::identity::repositories::execution::guard::use_block(
		&mut access,
		run,
		local_only,
	)
	.await
	.map_err(Into::into);
	access.finish(result).await
}

/// The definitions a read of `run` leaves in the set a retained lease may admit from, for tests:
/// read under the Run's own authority, or under a lease inherited from it.
#[doc(hidden)]
pub async fn retained_after_read(
	store: &Store,
	identity: &SubjectIdentity,
	subjects: Vec<String>,
	run: Uuid,
	inherited: bool,
) -> Result<std::collections::BTreeSet<(String, String)>> {
	let mut outer = Access::begin(store, identity).await?;
	outer.subjects = subjects;
	let mut access = if inherited {
		Access::under_lease(&outer).await?
	} else {
		outer
	};
	let _ = access.run_for_action(run).await;
	let retained = access.approved_catalog.clone();
	access.finish(Ok(())).await?;
	Ok(retained)
}

pub async fn claim(
	f: &Federation,
	identity: &SubjectIdentity,
	task: Uuid,
	revision: i64,
	agent: &EntityRef,
) -> Result<Task> {
	let mut access = Access::begin(&f.store, identity).await?;
	let result = admit(f, &mut access, task, Some(revision), agent, false).await;
	let result = access.finish(result).await?;
	f.notify.notify_waiters();
	Ok(result)
}

pub async fn delegate(
	f: &Federation,
	identity: &SubjectIdentity,
	task: Uuid,
	node: &str,
	agent: &EntityRef,
) -> Result<Delegation> {
	let target = (node != f.config.deployment)
		.then(|| aidash_domain::nodes::NodeAddress::parse(node))
		.transpose()?;
	if target
		.as_ref()
		.is_some_and(|target| target.deployment != f.config.deployment)
	{
		return super::remote::execution::delegate(f, identity, task, node, agent).await;
	}
	let mut access = Access::begin(&f.store, identity).await?;
	let result = async {
		let current = access.task_read(task).await?;
		let home = access.thread_node(current.thread_id).await?;
		if target.as_ref().is_none_or(|target| target.node == home) {
			delegate_in(f, &mut access, task, agent).await.map(Some)
		} else {
			Ok(None)
		}
	}
	.await;
	let result = access.finish(result).await?;
	match result {
		Some(result) => {
			f.notify.notify_waiters();
			Ok(result)
		}
		None => super::remote::execution::delegate(f, identity, task, node, agent).await,
	}
}

pub(crate) async fn delegate_in(
	f: &Federation,
	access: &mut Access,
	task: Uuid,
	agent: &EntityRef,
) -> Result<Delegation> {
	aidash_application::authorization::execution::admission::delegate(
		&mut crate::bootstrap::execution_admission_scope(f, access),
		task,
		agent,
	)
	.await
	.map_err(Into::into)
}

#[derive(Clone)]
pub(crate) struct WorkerAuthority {
	access: Arc<RwLock<Access>>,
	remote: Option<Federation>,
}

/// A Run's worker authority as its step holds it, for integration tests outside
/// this crate (#759). A test begins it while the Run's Subnode is active, ends
/// the Subnode, and then calls a path that stands in for a call that passed the
/// tool's entrance before the end. Production code gets the same authority from
/// `Guard`.
#[doc(hidden)]
pub struct StepAuthority(WorkerAuthority);

impl StepAuthority {
	/// The authority of `run`. Its outer transaction is released at once, as it is
	/// while a step waits on a tool, so that an end is never held up by it.
	pub async fn begin(f: &Federation, run: &Run) -> Result<Self> {
		let mut access = access_for_run(&f.store, &run.metadata(), true)
			.await?
			.ok_or(Error::Forbidden)?;
		access.suspend().await?;
		Ok(Self(WorkerAuthority {
			access: Arc::new(RwLock::new(access)),
			remote: None,
		}))
	}

	pub async fn sampling_approval(
		&self,
		store: &Store,
		run: &Run,
		request: aidash_application::capabilities::approvals::SamplingApprovalRequest,
	) -> Result<aidash_application::capabilities::approvals::ToolCallApproval> {
		self.0.sampling_approval(store, run, request).await
	}

	/// A Live preview start that passed the tool's admission.
	pub async fn live_preview_start(
		&self,
		store: &Store,
		run: &Run,
		command: &str,
		port: u16,
	) -> Result<Value> {
		let input = aidash_application::preview::live::Start {
			command: command.to_owned(),
			port,
		};
		self.0.live_preview_start(store, run, input).await
	}
}

fn unavailable_thread_search(thread: Uuid, disabled: bool) -> Value {
	let mut result = json!({"thread_id":thread, "matches":[], "estimated_tokens":0,
		"truncated":false, "thread":{"status":"unavailable", "cause":if disabled {"disabled"} else {"missing"}}});
	result["estimated_tokens"] =
		json!(aidash_domain::context::estimated_tokens(&result.to_string()) + 16);
	result
}

fn append_reference_matches(result: &mut Value, reference: Value, max_tokens: usize) -> Result<()> {
	let reference = reference
		.as_array()
		.cloned()
		.ok_or_else(|| Error::Invalid("invalid reference matches".into()))?;
	let thread_tokens = result["estimated_tokens"].as_u64().unwrap_or(0) as usize;
	let (reference, truncated) = aidash_domain::registry::knowledge::fit_reference_matches(
		reference,
		thread_tokens,
		max_tokens,
	);
	result["reference_matches"] = json!(reference);
	result["reference_truncated"] = json!(truncated);
	Ok(())
}

#[cfg(test)]
mod image_search_tests {
	use super::*;
	#[test]
	fn missing_or_disabled_thread_keeps_the_separate_reference_results() {
		for disabled in [false, true] {
			let mut result = unavailable_thread_search(Uuid::nil(), disabled);
			append_reference_matches(
				&mut result,
				json!([{"document":{"name":"diagram.png"}}]),
				1000,
			)
			.unwrap();
			assert_eq!(result["matches"], json!([]));
			assert_eq!(result["thread"]["status"], "unavailable");
			assert_eq!(
				result["thread"]["cause"],
				if disabled { "disabled" } else { "missing" }
			);
			assert_eq!(
				result["reference_matches"][0]["document"]["name"],
				"diagram.png"
			);
		}
	}
}

impl WorkerAuthority {
	pub(crate) async fn is_subnode_member(&self, run: &Run) -> Result<bool> {
		let mut none = std::collections::BTreeMap::<String, ()>::new();
		aidash_application::authorization::tools::filter_subnode(
			&crate::bootstrap::agent_tool_repository(None, &self.access, run),
			&mut none,
			|_| unreachable!("empty map"),
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn thread_search(
		&self,
		store: &Store,
		run: &Run,
		input: Value,
	) -> Result<Value> {
		let mut access = self.access.write().await;
		// A search leaves a Node that did not answer alone for a while, like the
		// lists a person reads; the Run's other reads still ask it (#770, Q16).
		let lists = std::mem::replace(&mut access.lists, true);
		let result = Self::thread_search_in(&mut access, store, run, input).await;
		access.lists = lists;
		result
	}
	async fn thread_search_in(
		access: &mut Access,
		store: &Store,
		run: &Run,
		input: Value,
	) -> Result<Value> {
		let attachment = if let Some(selection) = input.get("image").filter(|v| !v.is_null()) {
			let selection: aidash_domain::media::ThreadAttachmentSelection =
				serde_json::from_value(selection.clone())?;
			Some(aidash_domain::semantic::retrieval::AttachmentImageQuery {
				run_id: run.id,
				message_id: selection.message_id,
				attachment_id: selection.attachment_id,
				expected_digest: selection.expected_digest,
			})
		} else {
			None
		};
		let mut lease = crate::semantic::service::Lease::Inherited(access);
		lease.durable();
		lease.thread(run.thread_id, "semantic.search").await?;
		let reference = run
			.context
			.binding_snapshot
			.as_deref()
			.and_then(|snapshot| {
				snapshot
					.definitions
					.iter()
					.find(|entry| entry.identity == snapshot.agent)
			})
			.and_then(|entry| entry.definition.config.get("reference_embedding"))
			.filter(|value| !value.is_null())
			.map(|value| {
				serde_json::from_value::<aidash_domain::registry::EntityRef>(value.clone())
			})
			.transpose()?;
		if let Some(reference) = &reference {
			aidash_application::generation::embedding::require_reference_policy(
				&mut crate::bootstrap::generation_embedding_authority_scope(
					lease.access().ok_or(Error::Forbidden)?,
				),
				store.deployment.as_str(),
				reference,
			)
			.await?;
		}
		let index = crate::apps::knowledge::models::SemanticIndexe::locked(
			&mut **lease.tx(),
			run.thread_id,
			false,
		)
		.await?;
		let spec: Option<aidash_domain::semantic::indexing::IndexingSpec> = index
			.map(|index| serde_json::from_value(index.spec))
			.transpose()?;
		let limit = input["limit"].as_u64().map_or(5, |value| value as usize);
		let max_tokens = input["max_tokens"]
			.as_u64()
			.map_or(4096, |value| value as usize);
		if limit == 0 || limit > 20 || max_tokens == 0 || max_tokens > 32768 {
			return Err(Error::Invalid("invalid semantic search limits".into()));
		}
		let search = crate::apps::knowledge::serializers::contracts::Search {
			query: input["query"].as_str().unwrap_or_default().to_owned(),

			attachment: attachment.clone(),
			agent: None,
			metadata: json!({}),
			limit: input["limit"].as_u64().map_or_else(
				|| spec.as_ref().map_or(5, |spec| spec.max_results.min(5)),
				|value| {
					spec.as_ref().map_or(value as usize, |spec| {
						(value as usize).min(spec.max_results)
					})
				},
			),
			max_tokens: input["max_tokens"].as_u64().map_or_else(
				|| spec.as_ref().map_or(4096, |spec| spec.max_result_tokens),
				|value| {
					spec.as_ref().map_or(value as usize, |spec| {
						(value as usize).min(spec.max_result_tokens)
					})
				},
			),
		};
		let mut result = if spec.as_ref().is_some_and(|spec| spec.enabled) {
			serde_json::to_value(
				crate::semantic::service::search_in(
					store,
					&mut lease,
					run.thread_id,
					search,
					Some(run.id),
					None,
				)
				.await?,
			)?
		} else {
			unavailable_thread_search(run.thread_id, spec.is_some())
		};
		if reference.is_some() {
			use aidash_application::ports::semantic::retrieval::SemanticRetrievalSession;
			let limit = limit.min(8);
			// The same image query, read under the same rules as the Thread
			// search, against this agent version's private Reference Documents.
			let max_bytes = aidash_domain::semantic::indexing::default_max_image_bytes();
			let read = async |lease: &mut crate::semantic::service::Lease<'_>| -> Result<
				Option<aidash_application::ports::semantic::IndexImage>,
			> {
				let mut scope = crate::bootstrap::semantic_retrieval_scope(store, lease);
				Ok(if let Some(attachment) = &attachment {
					if !scope.attachment_seen(attachment).await? {
						return Err(
							aidash_application::semantic::retrieval::attachment_not_seen().into(),
						);
					}
					Some(
						scope
							.query_attachment(run.thread_id, attachment, max_bytes)
							.await?,
					)
				} else {
					None
				})
			};
			let reference = if let Some(content) = read(&mut lease).await? {
				let found = crate::apps::registry::services::knowledge::reference_image_context(
					lease.access().ok_or(Error::Forbidden)?,
					store,
					run,
					aidash_domain::semantic::EmbeddingInput::Image {
						media_type: &content.media_type,
						bytes: &content.bytes,
					},
					limit,
				)
				.await?;
				let current = read(&mut lease).await?.ok_or(Error::Forbidden)?;
				if current.media_type != content.media_type || current.bytes != content.bytes {
					return Err(Error::Conflict(
						"image query changed during retrieval".into(),
					));
				}
				found
			} else {
				crate::apps::registry::services::knowledge::reference_image_context(
					lease.access().ok_or(Error::Forbidden)?,
					store,
					run,
					aidash_domain::semantic::EmbeddingInput::Text(
						input["query"].as_str().unwrap_or_default(),
					),
					limit,
				)
				.await?
			};
			append_reference_matches(
				&mut result,
				reference.unwrap_or_else(|| json!([])),
				max_tokens,
			)?;
		}
		aidash_domain::unconfirmed::for_tool(&mut result);
		Ok(result)
	}
	/// The Envelope an Agent reads for a call whose effect needed no transaction of its own.
	async fn shaped(
		&self,
		store: &Store,
		run: &Run,
		area: &aidash_domain::capabilities::sessions::Area,
		key: &str,
		result: Value,
	) -> Result<Value> {
		let outer = self.access.read().await;
		let mut access = Access::under_lease(&outer).await?;
		let shaped = aidash_application::capabilities::files::envelope(
			&mut crate::bootstrap::file_scope(Some(store), &mut access, Some(run)),
			area,
			key,
			result,
		)
		.map_err(Error::from);
		access.finish(shaped).await.map(|envelope| {
			// The wire Envelope flattens the result beside its own fields.
			let mut wire = match envelope.result {
				Value::Object(fields) => fields,
				_ => Default::default(),
			};
			wire.insert("operation_id".into(), json!(envelope.operation_id));
			wire.insert("status".into(), json!(envelope.status));
			wire.insert("policy_revision".into(), json!(envelope.policy_revision));
			wire.insert("area_id".into(), json!(envelope.area_id));
			wire.insert("generation".into(), json!(envelope.generation));
			wire.insert("revision".into(), json!(envelope.revision));
			Value::Object(wire)
		})
	}

	/// `outbound_get` by a Run executing here for another Home (ADR 0139, decision 22). The
	/// Home's person at the root of the chain is asked first; only an explicit approval makes
	/// the read `approved` here, where the existing worker fetches it under this Node's own
	/// origins and authority. A repeat of the call (the same key) reads the Home's answer.
	pub(crate) async fn outbound_to_home(
		&self,
		store: &Store,
		run: &Run,
		home: &Home,
		input: Value,
		key: &str,
	) -> Result<Value> {
		use aidash_application::capabilities::approvals::{
			HomeAnswer, HomeOutbound, home_begin, home_settle,
		};
		use aidash_application::ports::capabilities::files::FileScopePort as _;
		let metadata = run.metadata();
		let begun = {
			let outer = self.access.read().await;
			let mut access = Access::under_lease(&outer).await?;
			let result: Result<_> = async {
				crate::apps::execution::repositories::subnodes::require_writable(
					access.tx.native_mut(),
					run.thread_id,
				)
				.await?;
				let (input, area) = {
					let mut scope =
						crate::bootstrap::file_scope(Some(store), &mut access, Some(run));
					let input = aidash_application::capabilities::files::admit_tool(
						&mut scope,
						&metadata,
						"outbound_get",
						input,
					)
					.await?;
					let area = scope.for_run(&metadata).await?;
					(input, area)
				};
				let outbound: aidash_domain::capabilities::approvals::Outbound =
					serde_json::from_value(input)
						.map_err(|error| aidash_application::Error::Invalid(error.to_string()))?;
				let mut scope = crate::bootstrap::approval_scope(Some(store), &mut access);
				scope.run = Some(Box::new(run.clone()));
				let begun = home_begin(&mut scope, &metadata, area.id, outbound).await?;
				Ok((area, begun))
			}
			.await;
			access.finish(result).await
		};
		let (area, begun) = match begun {
			Ok(begun) => begun,
			Err(error) => return blocked(key, error),
		};
		let ask = match begun {
			HomeOutbound::Settled(view) => return self.shaped(store, run, &area, key, view).await,
			HomeOutbound::Ask(ask) => ask,
		};
		// The Home's person is asked outside any transaction of this Node.
		let answer = match ask.home_request {
			None => match home.forward_outbound_request(&ask.prompt, &ask.key).await {
				Ok(request) => HomeAnswer::Asked(request.id),
				Err(error) => return blocked(key, error),
			},
			Some(id) => match home.forward_outbound_read(id).await {
				Ok(request) => HomeAnswer::Read {
					request: id,
					verdict: aidash_domain::capabilities::approvals::home_verdict(
						request.response.as_ref(),
						request.answered_by.as_deref(),
					),
				},
				Err(error) => return blocked(key, error),
			},
		};
		let settled = {
			let outer = self.access.read().await;
			let mut access = Access::under_lease(&outer).await?;
			let mut scope = crate::bootstrap::approval_scope(Some(store), &mut access);
			scope.run = Some(Box::new(run.clone()));
			let result = home_settle(&mut scope, &metadata, ask.record, answer)
				.await
				.map_err(Error::from);
			access.finish(result).await
		};
		match settled {
			Ok(view) => self.shaped(store, run, &area, key, view).await,
			Err(error) => blocked(key, error),
		}
	}

	/// `thread_message` with files by a Run executing here for another Home (ADR 0139,
	/// decision 20): the Run's files are snapshotted here as a local message's are, and the
	/// Home stores them as Thread Attachments of the message it posts for the Agent. No
	/// transaction is held while the Home takes them; the receipt is cached under the call's key
	/// after, so a repeat sends nothing twice.
	#[allow(clippy::too_many_arguments)]
	pub(crate) async fn message_files_to_home(
		&self,
		store: &Store,
		run: &Run,
		home: &Home,
		key: &str,
		content: &str,
		paths: &[String],
		descriptions: &std::collections::BTreeMap<String, String>,
	) -> Result<()> {
		use sha2::{Digest, Sha256};
		let receipt_key = {
			let mut hash = Sha256::new();
			hash.update(run.id.as_bytes());
			hash.update(key.as_bytes());
			hash.update(9_u64.to_be_bytes());
			let digest = hash.finalize();
			Uuid::from_bytes(digest[..16].try_into().expect("digest has 16 bytes"))
		};
		let receipt_digest = crate::registry::digest(&json!([
			"thread_message_files",
			run.id,
			content,
			paths,
			descriptions
		]));
		let snapshots = {
			let outer = self.access.read().await;
			let mut access = Access::under_lease(&outer).await?;
			let result: Result<_> = async {
				crate::apps::execution::repositories::subnodes::require_writable(
					access.tx.native_mut(),
					run.thread_id,
				)
				.await?;
				aidash_application::capabilities::files::snapshot_message_files_once(
					&mut crate::bootstrap::file_scope(Some(store), &mut access, Some(run)),
					&run.metadata(),
					receipt_key,
					&receipt_digest,
					false,
					paths,
				)
				.await
				.map_err(Into::into)
			}
			.await;
			access.finish(result).await?
		};
		// A cached receipt means the message was already sent.
		let Some(snapshots) = snapshots else {
			return Ok(());
		};
		if let Some((file, _)) = snapshots.iter().find(|(file, _)| {
			file.media_type.starts_with("image/") && !descriptions.contains_key(&file.path)
		}) {
			return Err(Error::Invalid(format!(
				"{} is an image: give it an Image Description in image_descriptions",
				file.path
			)));
		}
		let attached = descriptions
			.iter()
			.filter(|(path, _)| paths.contains(*path))
			.map(|(path, text)| (path.clone(), text.clone()))
			.collect();
		home.deliver(
			&format!("message:{key}"),
			&aidash_domain::federation::delivery::Purpose::Message {
				content: content.to_owned(),
				descriptions: attached,
			},
			&snapshots,
		)
		.await
		.map_err(after_effect)?;
		let outer = self.access.read().await;
		let mut access = Access::under_lease(&outer).await?;
		let cached = crate::apps::execution::capabilities::sessions::cache(
			&mut access,
			receipt_key,
			&receipt_digest,
			&json!({"sent":true}),
		)
		.await;
		access.finish(cached).await
	}

	pub(crate) async fn message_with_files(
		&self,
		store: &Store,
		run: &Run,
		key: &str,
		content: &str,
		paths: &[String],
		descriptions: &std::collections::BTreeMap<String, String>,
	) -> Result<()> {
		use sha2::{Digest, Sha256};
		fn operation_id(run: Uuid, key: &str, position: usize) -> Uuid {
			let mut hash = Sha256::new();
			hash.update(run.as_bytes());
			hash.update(key.as_bytes());
			hash.update((position as u64).to_be_bytes());
			let digest = hash.finalize();
			Uuid::from_bytes(digest[..16].try_into().expect("digest has 16 bytes"))
		}
		let receipt_key = operation_id(run.id, key, 9);
		let receipt_digest = if descriptions.is_empty() {
			crate::registry::digest(&json!(["thread_message_files", run.id, content, paths]))
		} else {
			crate::registry::digest(&json!([
				"thread_message_files",
				run.id,
				content,
				paths,
				descriptions
			]))
		};
		let agent = run.executor_subject(store.deployment.as_str());
		let outer = self.access.read().await;
		let mut access = Access::under_lease(&outer).await?;
		let prepare: Result<_> = async {
			// First, before the replay check below, so that this transaction holds
			// the Subnode lock through the upload and the post. A retry after the
			// Subnode ended is refused even when the first attempt was recorded.
			crate::apps::execution::repositories::subnodes::require_writable(
				access.tx.native_mut(),
				run.thread_id,
			)
			.await?;
			let thread = access.thread(run.thread_id).await?;
			access.require(&thread, "message.create").await?;
			// Older successful posts predate the receipt below. The invocation
			// journal has already checked this key's complete original input.
			let principal = crate::registry::digest(
				&json!({"tenant":access.identity.tenant,"subject":access.identity.subject}),
			);
			let principal =
				crate::registry::digest(&json!({"principal":principal,"run_id":run.id}));
			let message_key = format!(
				"channel:{}:{}:{}",
				run.thread_id,
				principal,
				operation_id(run.id, key, 8)
			);
			use reinhardt::query::{
				Alias, Expr, ExprTrait as _, PostgresQueryBuilder, Query,
				QueryStatementBuilder as _,
			};
			let previous: Option<(Uuid, String, String, Option<Uuid>)> =
				crate::database::native::query_as(
					&Query::select()
						.columns(["thread_id", "sender", "content", "run_id"].map(Alias::new))
						.from(Alias::new("messages"))
						.and_where(Expr::col("idempotency_key").eq(Expr::value(message_key)))
						.to_string(PostgresQueryBuilder),
				)
				.columns(&["thread_id", "sender", "content", "run_id"])
				.fetch_optional(&mut **access.tx)
				.await?;
			if let Some((thread, sender, original, publisher)) = &previous
				&& (*thread != run.thread_id
					|| sender != &agent
					|| original != content
					|| *publisher != Some(run.id))
			{
				return Err(Error::Conflict("message idempotency key reused".into()));
			}
			aidash_application::capabilities::files::snapshot_message_files_once(
				&mut crate::bootstrap::file_scope(Some(store), &mut access, Some(run)),
				&run.metadata(),
				receipt_key,
				&receipt_digest,
				previous.is_some(),
				paths,
			)
			.await
			.map_err(Into::into)
		}
		.await;
		let snapshots = match prepare {
			Ok(Some(snapshots)) => snapshots,
			Ok(None) => return access.finish(Ok(())).await,
			Err(error) => return access.finish(Err(error)).await,
		};
		// An Agent describes every image it posts, so people and Agents that
		// cannot see it can still find and understand it (#622 decision 7).
		if let Some((file, _)) = snapshots.iter().find(|(file, _)| {
			file.media_type.starts_with("image/") && !descriptions.contains_key(&file.path)
		}) {
			let missing = Error::Invalid(format!(
				"{} is an image: give it an Image Description in image_descriptions",
				file.path
			));
			return access.finish(Err(missing)).await;
		}
		let mut lease = crate::collaboration::access::Lease::from_access(access);
		let result: Result<()> = async {
			let mut ids = Vec::with_capacity(snapshots.len());
			for (index, (file, bytes)) in snapshots.iter().enumerate() {
				let filename = file.path.rsplit('/').next().unwrap_or("").to_owned();
				let input = crate::collaboration::ChannelAttachmentUploadQuery {
					filename,
					media_type: file.media_type.clone(),
					idempotency_key: operation_id(run.id, key, index),
					description: descriptions.get(&file.path).cloned(),
				};
				let attachment = crate::collaboration::attachments::upload_for_run(
					store,
					&mut lease,
					run.thread_id,
					input,
					bytes,
					Some(run.id),
				)
				.await?;
				ids.push(attachment.id);
			}
			let mut scope = crate::bootstrap::channel_scope(store, &mut lease);
			scope.run_id = Some(run.id);
			scope.agent = Some(agent);
			aidash_application::threads::channels::post(
				&mut scope,
				run.thread_id,
				aidash_application::threads::channels::MessageSubmission {
					content,
					idempotency_key: operation_id(run.id, key, 8),
					attachment_ids: &ids,
					citations: &[],
				},
			)
			.await?;
			crate::apps::execution::capabilities::sessions::cache(
				lease.access_mut(),
				receipt_key,
				&receipt_digest,
				&json!({"sent":true}),
			)
			.await?;
			Ok(())
		}
		.await;
		lease.finish(result).await
	}
	pub(crate) async fn copy_thread_attachment(
		&self,
		store: &Store,
		run: &Run,
		input: Value,
	) -> Result<Value> {
		use crate::apps::{
			execution::{
				capabilities::sessions,
				repositories::{operations::pods, subnodes},
			},
			workspaces::repositories::scratch,
		};
		use aidash_application::{capabilities::pod::files, scratch::copy_would_disclose};
		use aidash_domain::registry::rules::validate_path;

		let attachment_id: Uuid = serde_json::from_value(input["id"].clone())
			.map_err(|_| Error::Invalid("invalid attachment id".into()))?;
		let key: Uuid = serde_json::from_value(input["idempotency_key"].clone())
			.map_err(|_| Error::Invalid("invalid idempotency key".into()))?;
		let mut outer = self.access.write().await;
		let was_active = outer.tx.is_active();
		let result = async {
			if !was_active {
				crate::database::authority_change::enter(&store.pool, &outer.identity.tenant).await;
				self.refresh_pod_file_authority(&mut outer, store, run)
					.await?;
			}
			let _visibility = crate::transactions::gate::ReadLease::begin(store).await?;
			let access = Access::under_lease(&outer).await?;
			let mut lease = crate::collaboration::access::Lease::from_access(access);
			let result = async {
				let metadata = run.metadata();
				let access = lease.access_mut();
				subnodes::require_writable(access.tx.native_mut(), run.thread_id).await?;
				if metadata.phase.is_terminal() || run.control != aidash_domain::RunControl::Active
				{
					return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
				}
				let record = access
					.tool_thread_record(run.thread_id, run.thread_id, "attachment", attachment_id)
					.await?;
				let pod = pods::settle_file_context(store, access, &metadata)
					.await?
					.ok_or_else(|| {
						Error::Conflict("RUNTIME_UNAVAILABLE: no Execution Pod".into())
					})?;
				let space_id =
					Uuid::parse_str(pod.scratch_space_id()).map_err(|_| Error::Forbidden)?;
				let tenant = access.identity.tenant.clone();
				let space = scratch::read(&mut **access.tx, &tenant, space_id)
					.await?
					.ok_or(Error::Forbidden)?;
				let Some(readers) = scratch::copy_audience(access, &space).await? else {
					return Ok(copy_would_disclose());
				};
				for subject in readers {
					if !access
						.snapshot
						.bundle
						.subjects
						.get(&subject)
						.is_some_and(|value| value.enabled)
					{
						continue;
					}
					let audit = access.audit;
					let mut reader = access.scratch_reader_view(subject).await?;
					reader.audit = false;
					let readable = reader
						.tool_thread_record(
							run.thread_id,
							run.thread_id,
							"attachment",
							attachment_id,
						)
						.await;
					drop(reader);
					access.audit = audit;
					match readable {
						Ok(_) => {}
						Err(Error::NotFound(_) | Error::Forbidden) => {
							return Ok(copy_would_disclose());
						}
						Err(error) => return Err(error),
					}
				}
				let path = match input["path"].as_str() {
					Some(path) => path.to_owned(),
					None => {
						let filename =
							record["attachment"]["filename"].as_str().ok_or_else(|| {
								Error::Invalid("attachment filename is required".into())
							})?;
						validate_path(filename)?;
						format!("threads/{}/{filename}", run.thread_id)
					}
				};
				validate_path(&path)?;
				let digest = crate::registry::digest(&json!([
					"attachment_copy",
					run.id,
					attachment_id,
					path,
					record["sha256"]
				]));
				if let Some(cached) = sessions::cached(access, key, &digest).await? {
					return Ok(cached);
				}
				let (attachment, content) = aidash_application::threads::channels::download(
					&mut crate::bootstrap::channel_scope(store, &mut lease),
					run.thread_id,
					attachment_id,
				)
				.await?;
				let size = files::write(&pod, &run.id.to_string(), &path, &content, None).await?;
				let value = json!({
					"status":"completed", "location":"scratch", "path":path, "size":size,
					"digest":record["sha256"], "media_type":attachment.media_type,
					"source":{"message_id":record["message_id"],"attachment_id":attachment_id},
				});
				sessions::cache(lease.access_mut(), key, &digest, &value).await?;
				Ok(value)
			}
			.await;
			lease.finish(result).await
		}
		.await;
		if !was_active && outer.tx.is_active() {
			outer.suspend().await?;
		}
		result
	}

	/// The scope of every transaction this authority's Run opens.
	pub(crate) async fn tenancy(&self) -> Result<aidash_application::tenancy::TenancyScope> {
		self.access.read().await.tenancy()
	}
	/// Concurrent core tool calls of one Run share the outer authority lease.
	/// Each call runs in its own inner transaction while the outer transaction
	/// keeps the authorization locks, so revocation waits until every concurrent
	/// call ends. Exclusive users take the write lock. A `shared_read` call
	/// loads its Working Area with a shared row lock so batched reads overlap.
	pub async fn core_tool(
		&self,
		store: &Store,
		run: &Run,
		name: &str,
		input: Value,
		key: &str,
		shared_read: bool,
	) -> Result<Value> {
		if matches!(name, "live_preview_start" | "live_preview_stop") {
			return self.live_preview_tool(store, run, name, input).await;
		}
		use crate::capabilities::service::Invocation;
		if aidash_domain::tool::concurrency::pod_file_call(name, &input) {
			return match self.pod_file_tool(store, run, name, input, key).await {
				Ok(result) => Ok(result),
				Err(error) => blocked(key, error),
			};
		}
		let outer = self.access.read().await;
		match Self::prepare_core_tool(&outer, store, run, name, input, key, shared_read).await {
			Ok(Invocation::Complete(result)) => Ok(json!(result)),
			Ok(Invocation::Pod(_)) => Err(Error::Conflict(
				"Scratch Space file boundary required".into(),
			)),
			Err(error) => blocked(key, error),
		}
	}

	pub(crate) fn drive_tool<'a>(
		&'a self,
		store: &'a Store,
		run: &'a Run,
		name: &'a str,
		input: Value,
		key: &'a str,
	) -> futures_util::future::BoxFuture<'a, Result<Value>> {
		Box::pin(async move {
			if !run.is_local(store.deployment.as_str()) {
				return Err(Error::Forbidden);
			}
			let mut outer = self.access.write().await;
			let was_active = outer.tx.is_active();
			if !was_active {
				self.refresh_pod_file_authority(&mut outer, store, run)
					.await?;
			}
			let result = if name == "drive_publish" {
				let identity = outer.identity.clone();
				outer.suspend().await?;
				let fresh = access_for_run(store, &run.metadata(), false)
					.await?
					.ok_or(Error::Forbidden)?;
				if fresh.identity.subject != identity.subject
					|| fresh.identity.tenant != identity.tenant
				{
					return Err(Error::Forbidden);
				}
				crate::apps::drives::tools::invoke(store, fresh, run, name, input, key).await
			} else {
				let access = Access::under_lease(&outer).await?;
				crate::apps::drives::tools::invoke(store, access, run, name, input, key).await
			};
			if was_active && !outer.tx.is_active() {
				self.refresh_pod_file_authority(&mut outer, store, run)
					.await?;
			} else if !was_active && outer.tx.is_active() {
				outer.suspend().await?;
			}
			result.or_else(|error| blocked(key, error))
		})
	}

	fn core_tool_wire(result: aidash_domain::capabilities::files::Envelope) -> Result<Value> {
		Ok(serde_json::to_value(
			crate::capabilities::service::envelope(result)?,
		)?)
	}

	#[allow(clippy::too_many_arguments)]
	async fn prepare_core_tool(
		outer: &Access,
		store: &Store,
		run: &Run,
		name: &str,
		input: Value,
		key: &str,
		shared_read: bool,
	) -> Result<crate::capabilities::service::Invocation> {
		let mut access = Access::under_lease(outer).await?;
		access.shared_area = shared_read;
		let result = async {
			crate::apps::execution::repositories::subnodes::require_writable(
				access.tx.native_mut(),
				run.thread_id,
			)
			.await?;
			crate::capabilities::service::invoke(store, &mut access, run, name, input, key).await
		}
		.await;
		access.finish(result).await
	}

	async fn refresh_pod_file_authority(
		&self,
		outer: &mut Access,
		store: &Store,
		run: &Run,
	) -> Result<()> {
		if let Some(remote) = &self.remote {
			let (fresh, _) = super::peer::admission::worker_lease(remote, &run.metadata())
				.await?
				.ok_or(Error::Forbidden)?;
			*outer = fresh;
			return Ok(());
		}
		let result = refresh_access_for_run(outer, store, run).await;
		if result.is_err() {
			outer.discard_failed_execution_refresh().await;
		}
		result
	}

	/// A refreshed outer lease must remember the current Catalog entries before
	/// its inner tool transaction may use them.
	async fn remember_file_catalog(
		outer: &mut Access,
		run: &Run,
		tool: Option<&str>,
	) -> Result<()> {
		catalog::entry(
			outer,
			&EntityRef {
				id: run.agent_id.clone(),
				version: run.agent_version.clone(),
			},
			"agent.execute",
		)
		.await?;
		if let Some(tool) = tool {
			let binding = run
				.context
				.binding_snapshot
				.as_deref()
				.ok_or(Error::Forbidden)?
				.operation(tool)?;
			catalog::entry(outer, &binding.identity.local(), "registry.read").await?;
		}
		Ok(())
	}

	/// Restore current execution authority on both Runner outcomes.
	async fn without_pod_file_authority<T>(
		&self,
		outer: &mut Access,
		store: &Store,
		run: &Run,
		wait: impl std::future::Future<Output = aidash_application::Result<T>>,
	) -> Result<(T, crate::transactions::gate::ReadLease)> {
		let identity = outer.identity.clone();
		let subjects = outer.subjects.clone();
		let policy_revision = outer.snapshot.revision;
		outer.suspend().await?;
		let result = wait.await;
		// Settlement rechecks already-admitted work (ADR 0121).
		let visibility = crate::transactions::gate::ReadLease::begin(store)
			.await
			.map_err(after_effect)?;
		self.refresh_pod_file_authority(outer, store, run)
			.await
			.map_err(authority_gone_after_effect)?;
		if outer.identity.tenant != identity.tenant
			|| outer.identity.credential_id != identity.credential_id
			|| outer.identity.subject != identity.subject
			|| outer.subjects != subjects
		{
			return Err(authority_gone_after_effect(Error::Forbidden));
		}
		if outer.snapshot.revision != policy_revision {
			return Err(Error::Forbidden);
		}
		Ok((result?, visibility))
	}

	async fn pod_file_tool(
		&self,
		store: &Store,
		run: &Run,
		name: &str,
		input: Value,
		key: &str,
	) -> Result<Value> {
		use crate::apps::execution::repositories::{operations::remote_tools, subnodes};
		use crate::capabilities::service::Invocation;
		let mut outer = self.access.write().await;
		// The harness suspends its lease and visibility gate before these calls.
		// Calls from an already active boundary get their outer lease back below.
		let was_active = outer.tx.is_active();
		let result = async {
			if !was_active {
				crate::database::authority_change::enter(&store.pool, &outer.identity.tenant).await;
			}
			let mut visibility = crate::transactions::gate::ReadLease::begin(store).await?;
			if !was_active {
				self.refresh_pod_file_authority(&mut outer, store, run)
					.await?;
				Self::remember_file_catalog(&mut outer, run, Some(name)).await?;
			}
			let request =
				match Self::prepare_core_tool(&outer, store, run, name, input, key, false).await? {
					Invocation::Complete(result) => return Ok(json!(result)),
					Invocation::Pod(request) => request,
				};
			let metadata = run.metadata();
			visibility.suspend().await?;
			let (completed, _visibility) = self
				.without_pod_file_authority(&mut outer, store, run, request.execute(&metadata))
				.await?;
			Self::remember_file_catalog(&mut outer, run, Some(name)).await?;
			let mut access = Access::under_lease(&outer).await?;
			let result = async {
				subnodes::require_writable(access.tx.native_mut(), run.thread_id).await?;
				if remote_tools::withdrawn(access.tx.as_mut(), run.id, name).await? {
					return Err(Error::Forbidden);
				}
				let mut scope = crate::bootstrap::file_scope(Some(store), &mut access, Some(run));
				let envelope = completed
					.settle(&mut scope, &run.metadata(), name, key)
					.await?;
				Self::core_tool_wire(envelope)
			}
			.await;
			access.finish(result).await
		}
		.await;
		if !was_active && outer.tx.is_active() {
			outer.suspend().await?;
		}
		result
	}

	/// Start or stop the Run's Live preview. A start waits up to 120 seconds for
	/// the Pod's port, so it never holds an authority transaction across that
	/// wait: the row is committed, the Runner is dispatched, and a second
	/// transaction records the answer (ADR 0070).
	async fn live_preview_tool(
		&self,
		store: &Store,
		run: &Run,
		name: &str,
		input: Value,
	) -> Result<Value> {
		use crate::apps::execution::repositories::live_previews as previews;
		use aidash_domain::preview::live::StopReason;
		let domain = store.preview_domain.clone();
		if name == "live_preview_stop" {
			let previous = {
				let outer = self.access.read().await;
				let mut access = Access::under_lease(&outer).await?;
				let result = previews::status(store, &mut access, run, domain.clone()).await;
				access.finish(result).await?
			};
			let Some(previous) = previous else {
				return Ok(json!({"stopped": true, "live_preview": null}));
			};
			if previous.state.is_stopped() {
				return Ok(json!({"stopped": true, "live_preview": previous}));
			}
			let stopped = {
				let outer = self.access.read().await;
				let mut access = Access::under_lease(&outer).await?;
				let result = previews::request_stop(
					store,
					&mut access,
					run,
					domain.clone(),
					previous.id,
					StopReason::Stopped,
				)
				.await;
				access.finish(result).await?
			};
			if previews::terminate(store, &stopped).await.unwrap_or(false) {
				let outer = self.access.read().await;
				let mut access = Access::under_lease(&outer).await?;
				let result = previews::settle(store, &mut access, run, domain, previous.id).await;
				access.finish(result).await?;
			}
			return Ok(json!({"stopped": true, "live_preview": previous}));
		}
		let input = {
			let outer = self.access.read().await;
			let mut access = Access::under_lease(&outer).await?;
			let result = aidash_application::capabilities::files::admit_tool(
				&mut crate::bootstrap::file_scope(Some(store), &mut access, Some(run)),
				&run.metadata(),
				name,
				input,
			)
			.await;
			access.finish(result.map_err(Into::into)).await?
		};
		let input: aidash_application::preview::live::Start = serde_json::from_value(input)?;
		self.live_preview_start(store, run, input).await
	}

	/// Start the Run's Live preview from a request that passed admission. Once
	/// the Run's Subnode has ended neither the row nor the dispatch is written.
	async fn live_preview_start(
		&self,
		store: &Store,
		run: &Run,
		input: aidash_application::preview::live::Start,
	) -> Result<Value> {
		use crate::apps::execution::repositories::live_previews as previews;
		use aidash_domain::preview::live::StopReason;
		let domain = store.preview_domain.clone();
		if let Some(port) = store.preview_pod_port
			&& input.port != port
		{
			return Err(Error::Conflict(format!(
				"PREVIEW_PORT_NOT_ALLOWED: this Deployment admits Live previews on port {port}"
			)));
		}
		// Starting another replaces the Run's Live preview before a new row exists.
		let previous = {
			let outer = self.access.read().await;
			let mut access = Access::under_lease(&outer).await?;
			let result = previews::status(store, &mut access, run, domain.clone()).await;
			access.finish(result).await?
		};
		if let Some(previous) = previous
			&& !previous.state.is_stopped()
		{
			let replaced = {
				let outer = self.access.read().await;
				let mut access = Access::under_lease(&outer).await?;
				let result = previews::request_stop(
					store,
					&mut access,
					run,
					domain.clone(),
					previous.id,
					StopReason::Replaced,
				)
				.await;
				access.finish(result).await?
			};
			if previews::terminate(store, &replaced).await.unwrap_or(false) {
				let outer = self.access.read().await;
				let mut access = Access::under_lease(&outer).await?;
				let result =
					previews::settle(store, &mut access, run, domain.clone(), replaced.id).await;
				access.finish(result).await?;
			}
		}
		let pending = {
			let outer = self.access.read().await;
			let mut access = Access::under_lease(&outer).await?;
			let result = async {
				// Only a start is fenced: stopping, replacing and settling stay open
				// so that a Pod can still be stopped.
				crate::apps::execution::repositories::subnodes::require_writable(
					access.tx.native_mut(),
					run.thread_id,
				)
				.await?;
				previews::prepare(store, &mut access, run, domain.clone(), input.clone()).await
			}
			.await;
			access.finish(result).await?
		};
		let mut refused = false;
		let accepted = {
			let outer = self.access.read().await;
			let mut access = Access::under_lease(&outer).await?;
			let result = async {
				if let Err(error) =
					crate::apps::execution::repositories::subnodes::require_writable(
						access.tx.native_mut(),
						run.thread_id,
					)
					.await
				{
					refused = true;
					return Err(error);
				}
				// The preview runs in the Execution Pod as a Background process of
				// the Run, outside this transaction (#745).
				use crate::apps::execution::repositories::operations::pods;
				let metadata = run.metadata();
				let pod = pods::tool_context(store, &mut access, &metadata)
					.await?
					.ok_or_else(|| {
						Error::Conflict("RUNTIME_UNAVAILABLE: no isolated runner configured".into())
					})?;
				let directory = pods::working_directory(&mut access, &metadata).await?;
				Ok((pod, directory))
			}
			.await;
			access.finish(result).await
		};
		if refused {
			// The Subnode ended after the row was recorded, which marked it stopped.
			// Nothing reached the Runner, so no Pod proof will ever settle it: end it
			// as a start that never served, which releases its hold.
			let outer = self.access.read().await;
			let mut access = Access::under_lease(&outer).await?;
			let result = previews::finish_start(
				store,
				&mut access,
				run,
				domain.clone(),
				pending.id,
				previews::Dispatch::NotDispatched,
			)
			.await;
			let started = access.finish(result).await?;
			drop(outer);
			if started.status.state.is_stopped() {
				// Admission was refused before any dispatch, so no Pod can exist.
				let outer = self.access.read().await;
				let mut access = Access::under_lease(&outer).await?;
				let result =
					previews::settle(store, &mut access, run, domain.clone(), pending.id).await;
				access.finish(result).await?;
			}
		}
		let (pod, working_directory) = accepted?;
		let dispatched = match aidash_application::capabilities::pod::previews::start(
			&pod,
			pending.id,
			run.id,
			&input.command,
			pending.port,
			working_directory,
		)
		.await
		{
			Ok(host) => previews::Dispatch::Serving {
				pod_host: host,
				started_at: chrono::Utc::now(),
			},
			Err(_) => previews::Dispatch::Failed,
		};
		let outer = self.access.read().await;
		let mut access = Access::under_lease(&outer).await?;
		let result = previews::finish_start(
			store,
			&mut access,
			run,
			domain.clone(),
			pending.id,
			dispatched,
		)
		.await;
		let started = access.finish(result).await?;
		drop(outer);
		if started.status.state.is_stopped() {
			let confirmed = match previews::terminate(store, &pending).await {
				Ok(true) => true,
				Ok(false) => {
					tracing::warn!(preview_id = %pending.id, "live preview termination is not confirmed; the sweep will retry");
					false
				}
				Err(error) => {
					tracing::warn!(preview_id = %pending.id, %error, "could not terminate stopped live preview; the sweep will retry");
					false
				}
			};
			if confirmed {
				let outer = self.access.read().await;
				let mut access = Access::under_lease(&outer).await?;
				let result = previews::settle(store, &mut access, run, domain, pending.id).await;
				access.finish(result).await?;
			}
		}
		Ok(json!(started))
	}

	/// Admit one exact call of a Tenant's HTTP or MCP tool or of page reading, or open the Tool call
	/// approval the Run waits for, under the worker's current authority lease. It
	/// shares the capability approval records, decision API and Run waiting.
	pub async fn tool_call_approval(
		&self,
		store: &Store,
		run: &Run,
		request: aidash_application::capabilities::approvals::ToolCallRequest,
	) -> Result<aidash_application::capabilities::approvals::ToolCallApproval> {
		let outer = self.access.write().await;
		let mut access = Access::under_lease(&outer).await?;
		let result = async {
			if crate::apps::execution::repositories::operations::remote_tools::is_remote_member(
				access.tx.as_mut(),
				run.id,
			)
			.await?
			{
				// The Home's person answers only the creating Run's own requests (ADR 0110).
				return Err(Error::Refused(
					"a Subnode member of a Run started by a Remote Grant cannot ask a person"
						.into(),
				));
			}
			crate::apps::execution::repositories::subnodes::require_writable(
				access.tx.native_mut(),
				run.thread_id,
			)
			.await?;
			// A core Run's approval belongs to its Working Area, so its Thread
			// lists it; other Runs have none.
			let area =
				crate::apps::execution::repositories::sessions::run_area(&mut access, run.id)
					.await?
					.flatten()
					.map(|area| area.id);
			let request =
				aidash_application::capabilities::approvals::ToolCallRequest { area, ..request };
			let mut scope = crate::bootstrap::approval_scope(Some(store), &mut access);
			scope.run = Some(Box::new(run.clone()));
			aidash_application::capabilities::approvals::tool_call(
				&mut scope,
				&run.metadata(),
				request,
			)
			.await
			.map_err(Error::from)
		}
		.await;
		access.finish(result).await
	}

	pub async fn sampling_approval(
		&self,
		store: &Store,
		run: &Run,
		request: aidash_application::capabilities::approvals::SamplingApprovalRequest,
	) -> Result<aidash_application::capabilities::approvals::ToolCallApproval> {
		let outer = self.access.write().await;
		let mut access = Access::under_lease(&outer).await?;
		let result = async {
			if crate::apps::execution::repositories::operations::remote_tools::is_remote_member(
				access.tx.as_mut(),
				run.id,
			)
			.await?
			{
				// The Home's person answers only the creating Run's own requests (ADR 0110).
				return Err(Error::Refused(
					"a Subnode member of a Run started by a Remote Grant cannot ask a person"
						.into(),
				));
			}
			crate::apps::execution::repositories::subnodes::require_writable(
				access.tx.native_mut(),
				run.thread_id,
			)
			.await?;
			// A core Run's approval belongs to its Working Area, so its Thread
			// lists it; other Runs have none.
			let area =
				crate::apps::execution::repositories::sessions::run_area(&mut access, run.id)
					.await?
					.flatten()
					.map(|area| area.id);
			let request = aidash_application::capabilities::approvals::SamplingApprovalRequest {
				area,
				..request
			};
			let mut scope = crate::bootstrap::approval_scope(Some(store), &mut access);
			aidash_application::capabilities::approvals::sampling::request(
				&mut scope,
				&run.metadata(),
				request,
			)
			.await
			.map_err(Error::from)
		}
		.await;
		access.finish(result).await
	}

	/// The Approval Mode that covers `run` now, under the worker's current
	/// authority lease: the one of the person its delegation chain starts at,
	/// for the Run's Thread. `None` when no person's mode covers the Run
	/// (ADR 0076).
	pub async fn approval_mode(
		&self,
		store: &Store,
		run: &Run,
	) -> Result<Option<aidash_domain::approval_rules::ApprovalMode>> {
		let outer = self.access.read().await;
		let mut access = Access::under_lease(&outer).await?;
		let result = async {
			let access = &mut access;
			crate::apps::identity::repositories::approval_preferences::covering(
				&mut *access.tx,
				&access.snapshot.bundle,
				store.deployment.as_str(),
				access.subjects.first().map(String::as_str),
				&run.metadata(),
			)
			.await
			.map(|covering| covering.map(|covering| covering.mode))
		}
		.await;
		access.finish(result).await
	}

	pub async fn connection_mode(
		&self,
		store: &Store,
		run: &Run,
	) -> Result<Option<aidash_domain::approval_rules::ApprovalMode>> {
		let outer = self.access.read().await;
		let mut access = Access::under_lease(&outer).await?;
		let result = async {
			use aidash_application::ports::capabilities::approvals::ApprovalScope;
			let mut scope = crate::bootstrap::approval_scope(Some(store), &mut access);
			let target = scope.resource(
				"run",
				&run.id.to_string(),
				serde_json::json!({"run_id":run.id}),
			);
			aidash_application::capabilities::approvals::connection_mode(
				&mut scope,
				&run.metadata(),
				&target,
			)
			.await
			.map_err(Error::from)
		}
		.await;
		access.finish(result).await
	}

	/// Whether Auto is this Run's effective Approval Mode now, under the
	/// worker's current authority lease.
	pub async fn auto_judging(&self, store: &Store, run: &Run) -> Result<bool> {
		let outer = self.access.read().await;
		let mut access = Access::under_lease(&outer).await?;
		let result = async {
			let mut scope = crate::bootstrap::approval_scope(Some(store), &mut access);
			aidash_application::capabilities::approvals::auto_judging(&mut scope, &run.metadata())
				.await
				.map_err(Error::from)
		}
		.await;
		access.finish(result).await
	}

	/// The Answer Deadline, in seconds, of the person whose delegation chain
	/// `run` starts at, under the worker's current authority lease. `None` when
	/// they set none or no person's settings cover the Run (ADR 0076).
	pub async fn answer_deadline(&self, store: &Store, run: &Run) -> Result<Option<i64>> {
		let outer = self.access.read().await;
		let mut access = Access::under_lease(&outer).await?;
		let result = async {
			let access = &mut access;
			crate::apps::identity::repositories::approval_preferences::covering(
				&mut *access.tx,
				&access.snapshot.bundle,
				store.deployment.as_str(),
				access.subjects.first().map(String::as_str),
				&run.metadata(),
			)
			.await
			.map(|covering| covering.and_then(|covering| covering.answer_deadline_seconds))
		}
		.await;
		access.finish(result).await
	}

	/// The pending approval of `run` that the approval Decider is to judge, read
	/// under the worker's current authority lease (ADR 0077).
	pub async fn approval_judgement(
		&self,
		store: &Store,
		run: &Run,
		approval: Uuid,
	) -> Result<Option<aidash_application::capabilities::approvals::JudgeRequest>> {
		let outer = self.access.read().await;
		let mut access = Access::under_lease(&outer).await?;
		let result = async {
			let mut scope = crate::bootstrap::approval_scope(Some(store), &mut access);
			aidash_application::capabilities::approvals::judgement(
				&mut scope,
				&run.metadata(),
				approval,
			)
			.await
			.map_err(Error::from)
		}
		.await;
		access.finish(result).await
	}

	/// Record how the approval Decider settled `approval`, in the name of the
	/// person the Run's chain starts at.
	pub async fn settle_judgement(
		&self,
		store: &Store,
		run: &Run,
		approval: Uuid,
		settlement: aidash_application::capabilities::approvals::Settlement,
	) -> Result<bool> {
		let outer = self.access.write().await;
		let mut access = Access::under_lease(&outer).await?;
		let result = async {
			let mut scope = crate::bootstrap::approval_scope(Some(store), &mut access);
			aidash_application::capabilities::approvals::settle(
				&mut scope,
				&run.metadata(),
				approval,
				settlement,
			)
			.await
			.map_err(Error::from)
		}
		.await;
		access.finish(result).await
	}

	pub async fn memory_mutate(
		&self,
		store: &Store,
		run: &Run,
		key: &str,
		changes: &[aidash_domain::memory::Change],
	) -> Result<Vec<aidash_domain::memory::Unit>> {
		let mut outer = self.access.write().await;
		if outer.tx.is_active() {
			outer.suspend().await?;
		}
		let result = async {
			let mut access = access_for_run(store, &run.metadata(), true)
				.await?
				.ok_or(Error::Forbidden)?;
			memory_run_check(&mut access, &run.metadata()).await?;
			let mut lease = crate::semantic::service::Lease::Scoped(Box::new(access));
			let result =
				crate::semantic::native_memory::run_mutate(store, &mut lease, run, key, changes)
					.await;
			lease.finish(result).await
		}
		.await;
		restore_outer(&mut outer, store, run, result).await
	}
	pub async fn memory_recall(
		&self,
		store: &Store,
		run: &Run,
		key: &str,
		query: aidash_domain::memory::RecallQuery,
		reflect: bool,
	) -> Result<(
		crate::semantic::native_memory::Outcome,
		Vec<aidash_domain::unconfirmed::Unconfirmed>,
	)> {
		let mut outer = self.access.write().await;
		if outer.tx.is_active() {
			outer.suspend().await?;
		}
		let result = async {
			let mut access = access_for_run(store, &run.metadata(), true)
				.await?
				.ok_or(Error::Forbidden)?;
			memory_run_check(&mut access, &run.metadata()).await?;
			let mut lease = crate::semantic::service::Lease::Scoped(Box::new(access));
			lease.lists();
			let result = crate::semantic::native_memory::run_recall(
				store, &mut lease, run, key, query, reflect,
			)
			.await;
			let left_out = lease.take_unconfirmed().into_entries();
			lease
				.finish(result)
				.await
				.map(|outcome| (outcome, left_out))
		}
		.await;
		restore_outer(&mut outer, store, run, result).await
	}

	pub async fn memory_hydrate(
		&self,
		store: &Store,
		run: &Run,
		sources: &[aidash_domain::memory::Evidence],
	) -> Result<serde_json::Value> {
		let mut outer = self.access.write().await;
		if outer.tx.is_active() {
			outer.suspend().await?;
		}
		let result = async {
			let mut access = access_for_run(store, &run.metadata(), true)
				.await?
				.ok_or(Error::Forbidden)?;
			memory_run_check(&mut access, &run.metadata()).await?;
			let mut lease = crate::semantic::service::Lease::Scoped(Box::new(access));
			let result =
				crate::semantic::native_memory::run_hydrate(store, &mut lease, run, sources).await;
			lease.finish(result).await
		}
		.await;
		restore_outer(&mut outer, store, run, result).await
	}

	pub async fn memory_start_recall(
		&self,
		store: &Store,
		run: &Run,
		query: aidash_domain::memory::RecallQuery,
	) -> Result<crate::semantic::native_memory::Outcome> {
		let mut outer = self.access.write().await;
		if outer.tx.is_active() {
			outer.suspend().await?;
		}
		let result = async {
			let mut access = access_for_run(store, &run.metadata(), true)
				.await?
				.ok_or(Error::Forbidden)?;
			memory_run_check(&mut access, &run.metadata()).await?;
			let mut lease = crate::semantic::service::Lease::Scoped(Box::new(access));
			let result =
				crate::semantic::native_memory::run_start_recall(store, &mut lease, run, query)
					.await;
			lease.finish(result).await
		}
		.await;
		restore_outer(&mut outer, store, run, result).await
	}

	pub async fn assign(
		&self,
		f: &Federation,
		run: &Run,
		task: Uuid,
		policy: &str,
		reason: &str,
		draft: Option<&aidash_domain::generation::policy::Draft>,
	) -> Result<crate::generation::Assignment> {
		let mut lease = self.access.write().await;
		// Read potential references under the outer lease before opening the mutation transaction.
		catalog::list_in(&mut lease, &Search::default()).await?;
		let mut access = Access::under_lease(&lease).await?;
		let result = aidash_application::authorization::worker_tasks::assign(
			&mut crate::bootstrap::worker_task_scope(f, &mut access),
			&run.metadata(),
			task,
			policy,
			reason,
			draft,
		)
		.await
		.map_err(Into::into);
		access.finish(result).await
	}

	pub(crate) async fn create_subnode(
		&self,
		f: &Federation,
		run: &Run,
		key: &str,
		input: &aidash_application::subnodes::Create,
	) -> Result<aidash_domain::subnodes::Subnode> {
		use crate::apps::identity::repositories::subnodes::{
			Creation, binding_authority, check_member_bindings, check_member_lists,
			creator_can_use_binding,
		};
		let mut lease = self.access.write().await;
		let node_address = format!("{}/{}", lease.node_id, run.node);
		let creator =
			aidash_domain::agent_subject(&node_address, &run.agent_id, &run.agent_version);
		if lease.subjects.last() != Some(&creator) {
			return Err(Error::Forbidden);
		}
		let resource = lease.node_resource(&run.node).await?;
		let evaluation = lease.evaluation(
			&creator,
			&resource,
			aidash_domain::identity::tenant_administration::SUBNODE_CREATE,
		);
		let mut decision = lease.snapshot.bundle.evaluate(&evaluation);
		decision.revision = lease.snapshot.revision;
		let allowed = decision.allowed;
		lease.record(&[(evaluation, decision)]).await?;
		if !allowed {
			// This check evaluates the policy itself rather than through `Access::require`.
			aidash_application::authorization::refusal_note::set(
				aidash_application::authorization::refusal_note::Note::new(
					aidash_domain::tool::RefusalCheck::Policy,
					Some(aidash_domain::identity::tenant_administration::SUBNODE_CREATE),
				),
			);
			return Err(Error::Forbidden);
		}
		lease.set_node(run.node.clone());
		let snapshot = run
			.context
			.binding_snapshot
			.as_ref()
			.ok_or_else(|| Error::Invalid("Run has no Binding snapshot".into()))?;
		let mut usable = (**snapshot).clone();
		usable.bindings.clear();
		let config = aidash_domain::registry::AgentConfig::from_snapshot(snapshot)?;
		for binding in &snapshot.bindings {
			if binding.excluded_reason.is_some()
				|| binding.definition.kind != "tool"
				|| !creator_can_use_binding(&config, binding)?
			{
				continue;
			}
			let reference = binding.identity.local();
			match catalog::tool_entry(
				&mut lease,
				&reference,
				"tool.invoke",
				binding.alias.as_deref().unwrap_or(&reference.id),
			)
			.await
			{
				Ok(entry) => {
					if aidash_domain::registry::rules::digest(&serde_json::to_value(&entry)?)
						!= binding.digest
					{
						continue;
					}
					match crate::marketplace::check_pinned(&mut lease, &entry).await {
						Ok(()) => usable.bindings.push(binding.clone()),
						Err(Error::Forbidden | Error::NotFound(_) | Error::AuthorityRefused(_)) => {
						}
						Err(error) => return Err(error),
					}
				}
				Err(Error::Forbidden)
				| Err(Error::NotFound(_))
				| Err(Error::AuthorityRefused(_)) => {}
				Err(error) => return Err(error),
			}
		}
		let creator_authority = binding_authority(&usable, &config)?;
		let mut members = std::collections::BTreeMap::new();
		for agent in &input.members {
			let entry =
				catalog::tool_entry(&mut lease, agent, "agent.execute", "subnode_create").await?;
			crate::marketplace::check_pinned(&mut lease, &entry).await?;
			let snapshot = crate::apps::registry::repositories::bindings::authorized_child(
				&mut lease,
				&entry,
				f.store.provider_credentials.is_some(),
			)
			.await?;
			if snapshot
				.definitions
				.iter()
				.any(|definition| definition.definition.kind == "cluster")
			{
				return Err(Error::Refused(
					"Subnode members cannot bind a Cluster".into(),
				));
			}
			if !snapshot.agent_plugins.is_empty() {
				return Err(Error::Refused(
					"Agent Plugin tools cannot be constrained by a Subnode allowance".into(),
				));
			}
			let config = aidash_domain::registry::AgentConfig::from_snapshot(&snapshot)?;
			check_member_bindings(&usable, &snapshot, &config)?;
			// A creator that a Remote Grant started passes on only what both Nodes' lists give it.
			check_member_lists(&mut lease, run.id, &snapshot, &config).await?;
			members.insert(
				format!("{}@{}", agent.id, agent.version),
				binding_authority(&snapshot, &config)?,
			);
		}
		let mut access = Access::under_lease(&lease).await?;
		let result = aidash_application::subnodes::create(
			&mut Creation {
				access: &mut access,
				federation: f,
				node_address,
				creator_authority,
				members,
			},
			&run.metadata(),
			key,
			input,
		)
		.await
		.map_err(Into::into);
		access.finish(result).await
	}

	pub(crate) async fn subnode_command(
		&self,
		f: &Federation,
		run: &Run,
		key: &str,
		command: &aidash_application::subnodes::Command,
	) -> Result<Value> {
		use crate::apps::execution::models::subnodes::{SubnodeOperationReceipt, SubnodeRecord};
		use crate::apps::identity::repositories::subnodes::{
			Management, binding_authority, check_member_bindings, check_member_lists,
			creator_binding_authority,
		};
		use aidash_application::ports::subnodes::ManageScope as _;
		use aidash_application::subnodes::{self, Command};
		use reinhardt::db::orm::Model;
		let mut lease = self.access.write().await;
		let tenant = lease.identity.tenant.clone();
		let mut query =
			SubnodeRecord::objects().filter(SubnodeRecord::field_tenant().eq(tenant.clone()));
		query = match command {
			Command::Task { id, .. }
			| Command::Add { id, .. }
			| Command::Remove { id, .. }
			| Command::Dissolve { id } => query.filter(SubnodeRecord::field_id().eq(id.0)),
			Command::Message { .. } => {
				query.filter(SubnodeRecord::field_thread_id().eq(run.thread_id))
			}
		};
		let record = query
			.all_with_executor(lease.tx.native_mut())
			.await
			.map_err(reinhardt::core::exception::Error::from)?
			.pop()
			.ok_or(Error::Forbidden)?;
		let subnode = record.domain(lease.tx.native_mut()).await?;
		let is_creator = subnode.creator_run_id == run.id;
		let is_member = subnode.thread_id == run.thread_id
			&& subnode.members.iter().any(|member| {
				member.agent_id == run.agent_id && member.agent_version == run.agent_version
			});
		if !is_creator && !is_member {
			return Err(Error::Forbidden);
		}
		if matches!(command, Command::Task { .. } | Command::Dissolve { .. }) && !is_creator {
			return Err(Error::Forbidden);
		}
		if matches!(command, Command::Message { .. }) && !is_member {
			return Err(Error::Forbidden);
		}
		let tenancy = lease.identity.tenancy()?;
		let creator = f.store.run(&tenancy, subnode.creator_run_id).await?;
		let mut creator_authority = aidash_domain::subnodes::Allowance::default();
		let mut candidate_authority = None;
		let mut creator_bindings = None;
		if matches!(command, Command::Add { .. } | Command::Task { .. }) {
			lease.set_node(run.node.clone());
			let (authority, bindings) =
				creator_binding_authority(&mut lease, &creator, &subnode).await?;
			creator_authority = authority;
			creator_bindings = Some(bindings);
		}
		let candidate = match command {
			Command::Add { agent, .. } => Some((agent, Some("subnode_add_member"))),
			Command::Task { .. } => Some((&subnode.leader, None)),
			_ => None,
		};
		if let Some((candidate, tool)) = candidate {
			lease.set_node(run.node.clone());
			let entry = match tool {
				Some(tool) => {
					catalog::tool_entry(&mut lease, candidate, "agent.execute", tool).await?
				}
				None => catalog::entry(&mut lease, candidate, "agent.execute").await?,
			};
			crate::marketplace::check_pinned(&mut lease, &entry).await?;
			let snapshot = crate::apps::registry::repositories::bindings::authorized_child(
				&mut lease,
				&entry,
				f.store.provider_credentials.is_some(),
			)
			.await?;
			if snapshot
				.definitions
				.iter()
				.any(|definition| definition.definition.kind == "cluster")
			{
				return Err(Error::Refused(
					"Subnode members cannot bind a Cluster".into(),
				));
			}
			if !snapshot.agent_plugins.is_empty() {
				return Err(Error::Refused(
					"Agent Plugin tools cannot be constrained by a Subnode allowance".into(),
				));
			}
			let config = aidash_domain::registry::AgentConfig::from_snapshot(&snapshot)?;
			check_member_bindings(
				creator_bindings
					.as_ref()
					.expect("candidate has creator bindings"),
				&snapshot,
				&config,
			)?;
			// The creating Run's lists bound a member added or tasked later too, as they may
			// have narrowed since the Subnode was created.
			check_member_lists(&mut lease, creator.id, &snapshot, &config).await?;
			let required = binding_authority(&snapshot, &config)?;
			aidash_domain::subnodes::member_fits(&required, &subnode.allowance)
				.map_err(|error| Error::Refused(error.to_string()))?;
			candidate_authority = Some(required);
		}
		let message_limit = if matches!(command, Command::Message { .. }) {
			f.run_message_limit(&creator).await?
		} else {
			0
		};
		let mut access = Access::under_lease(&lease).await?;
		let mut scope = Management {
			access: &mut access,
			federation: f,
			creator_authority,
			candidate_authority,
			message_limit,
		};
		let locked = scope.lock(subnode.id).await?.ok_or(Error::Forbidden)?;
		let creator = locked.creator_run_id == run.id;
		let member = locked.thread_id == run.thread_id
			&& locked.members.iter().any(|member| {
				member.agent_id == run.agent_id && member.agent_version == run.agent_version
			});
		if (!member || matches!(command, Command::Task { .. } | Command::Dissolve { .. }))
			&& !creator
			|| (matches!(command, Command::Message { .. }) && !member)
		{
			return Err(Error::Forbidden);
		}
		let fingerprint = crate::registry::digest(&serde_json::to_value(command)?);
		if let Some(previous) = SubnodeOperationReceipt::replay(
			scope.access.tx.native_mut(),
			&tenant,
			run.id,
			key,
			&fingerprint,
		)
		.await?
		{
			return access.finish(Ok(previous)).await;
		}
		let result: Result<Value> = match command {
			Command::Task {
				id,
				title,
				request,
				goal,
				journal_range,
			} => {
				async {
					aidash_domain::goal::validate_goal(goal.as_deref())?;
					let task = subnodes::task(
						&mut scope,
						&run.metadata(),
						*id,
						key,
						title,
						request,
						*journal_range,
					)
					.await?;
					if let Some(goal) = goal {
						let child = crate::apps::workspaces::repositories::goals::locked_task(
							&mut **scope.access.tx,
							task,
						)
						.await?;
						let creator = child.created_by.clone();
						crate::apps::workspaces::repositories::goals::change(
							&mut **scope.access.tx,
							&child,
							&creator,
							crate::apps::workspaces::repositories::goals::Change {
								goal: Some(goal.clone()),
								expected_revision: child.revision,
								resume: false,
							},
						)
						.await?;
					}
					Ok(json!({"task_id":task}))
				}
				.await
			}
			Command::Add { id, agent } => {
				subnodes::add_member(&mut scope, &run.metadata(), *id, agent)
					.await
					.map(|()| json!({"added":true}))
					.map_err(Into::into)
			}
			Command::Remove { id, agent } => {
				subnodes::remove_member(&mut scope, &run.metadata(), *id, agent)
					.await
					.map(|()| json!({"removed":true}))
					.map_err(Into::into)
			}
			Command::Dissolve { id } => subnodes::dissolve(&mut scope, &run.metadata(), *id)
				.await
				.map(|()| json!({"dissolved":true}))
				.map_err(Into::into),
			Command::Message { content } => {
				subnodes::message(&mut scope, &run.metadata(), subnode.id, key, content)
					.await
					.map(|()| json!({"sent":true}))
					.map_err(Into::into)
			}
		};
		let result = match result {
			Ok(value) => {
				SubnodeOperationReceipt::save(
					scope.access.tx.native_mut(),
					&tenant,
					run.id,
					key,
					&fingerprint,
					value.clone(),
				)
				.await?;
				Ok(value)
			}
			Err(error) => Err(error),
		};
		let result = access.finish(result).await;
		if result.is_ok() {
			f.notify.notify_waiters();
		}
		result
	}

	pub async fn create_task(
		&self,
		f: &Federation,
		run: &Run,
		key: &str,
		input: &NewTask,
	) -> Result<Task> {
		let lease = self.access.write().await;
		let mut access = Access::under_lease(&lease).await?;
		let result = aidash_application::authorization::worker_tasks::create_task(
			&mut crate::bootstrap::worker_task_scope(f, &mut access),
			&run.metadata(),
			key,
			input,
		)
		.await
		.map_err(Into::into);
		access.finish(result).await
	}

	pub async fn snapshot(&self, thread: Uuid) -> Result<ThreadSnapshot> {
		self.access.write().await.thread_snapshot(thread).await
	}

	/// `exclude_run` leaves that Run's own events out (#942).
	pub async fn workspace_observation(
		&self,
		thread: Uuid,
		offset: usize,
		limit: usize,
		exclude_run: Option<Uuid>,
	) -> Result<Value> {
		let mut access = self.access.write().await;
		// An observation lists the Thread, so it leaves a Node that did not
		// answer alone for a while (#770, Q16).
		let lists = std::mem::replace(&mut access.lists, true);
		let result = access
			.workspace_observation(thread, offset, limit, exclude_run)
			.await;
		access.lists = lists;
		result
	}

	pub async fn workspace_observation_fitted<F>(
		&self,
		thread: Uuid,
		offset: usize,
		limit: usize,
		fits: F,
	) -> Result<Option<(usize, Value)>>
	where
		F: FnMut(usize, &Value) -> Result<bool>,
	{
		let mut access = self.access.write().await;
		// See `workspace_observation` (#770, Q16).
		let lists = std::mem::replace(&mut access.lists, true);
		let result = access
			.workspace_observation_fitted(thread, offset, limit, fits)
			.await;
		access.lists = lists;
		result
	}

	pub async fn skill_context(&self, store: &Store, run: &Run) -> Result<String> {
		let mut access = self.access.write().await;
		crate::capabilities::skills::context(store, &mut access, run).await
	}
	pub async fn skill_context_items(
		&self,
		store: &Store,
		run: &Run,
	) -> Result<Vec<aidash_application::capabilities::skills::ContextItem>> {
		let mut access = self.access.write().await;
		aidash_application::capabilities::skills::context_items(
			&mut crate::bootstrap::file_scope(Some(store), &mut access, Some(run)),
			&run.metadata(),
		)
		.await
		.map_err(Into::into)
	}
	pub async fn direct_skills(
		&self,
		store: &Store,
		run: &Run,
	) -> Result<Vec<aidash_domain::exposure::DirectSkill>> {
		let mut access = self.access.write().await;
		crate::capabilities::skills::direct(store, &mut access, run).await
	}
	pub async fn direct_skill_body(
		&self,
		store: &Store,
		run: &Run,
		skill_id: Uuid,
		digest: &str,
	) -> Result<Option<String>> {
		let mut access = self.access.write().await;
		crate::capabilities::skills::direct_body(store, &mut access, run, skill_id, digest).await
	}
	pub async fn direct_skill_file(
		&self,
		store: &Store,
		run: &Run,
		skill_id: Uuid,
		digest: &str,
		path: &str,
	) -> Result<Option<Vec<u8>>> {
		let mut access = self.access.write().await;
		crate::capabilities::skills::direct_file(store, &mut access, run, skill_id, digest, path)
			.await
	}
	pub(crate) async fn subnode_thread_record(
		&self,
		run: &Run,
		thread: Uuid,
		kind: &str,
		id: Uuid,
	) -> Result<Value> {
		use crate::apps::execution::models::subnodes::SubnodeRecord;
		use reinhardt::db::orm::Model;
		let mut lease = self.access.write().await;
		let tenant = lease.identity.tenant.clone();
		let matching = SubnodeRecord::objects()
			.filter(SubnodeRecord::field_tenant().eq(tenant))
			.filter(SubnodeRecord::field_thread_id().eq(thread))
			.filter(SubnodeRecord::field_creator_run_id().eq(run.id))
			.all_with_executor(lease.tx.native_mut())
			.await
			.map_err(reinhardt::core::exception::Error::from)?;
		if matching.is_empty() {
			use crate::apps::identity::repositories::visibility::records;
			if records::in_creator_thread(&mut lease, run.thread_id, kind, id).await? {
				return Err(records::creator_thread_boundary());
			}
			return Err(records::wrong_thread(kind, id));
		}
		let mut access = Access::under_lease(&lease).await?;
		// The creator keeps its own Thread and gains read access to this one;
		// the producers of the Subnode's records may have read in the creator's.
		access.context = json!({"thread_id":run.thread_id,"subnode_read_threads":[thread]});
		let result = access
			.tool_thread_record(run.thread_id, thread, kind, id)
			.await;

		access.finish(result).await
	}
	pub(crate) async fn tool_thread_record(
		&self,
		thread: Uuid,
		kind: &str,
		id: Uuid,
	) -> Result<Value> {
		self.access
			.write()
			.await
			.tool_thread_record(thread, thread, kind, id)
			.await
	}

	pub async fn thread_record(&self, thread: Uuid, kind: &str, id: Uuid) -> Result<Value> {
		self.access
			.write()
			.await
			.thread_record(thread, kind, id)
			.await
	}

	pub async fn thread_child_summary(
		&self,
		thread: Uuid,
		parent: Uuid,
	) -> Result<ChildTaskSummary> {
		self.access
			.write()
			.await
			.thread_children(thread, parent)
			.await
	}

	pub async fn delegate(
		&self,
		f: &Federation,
		run: &Run,
		task: Uuid,
		node: &str,
		agent: &EntityRef,
	) -> Result<Delegation> {
		let mut lease = self.access.write().await;
		if !run.is_local(f.config.deployment.as_str()) {
			return Err(Error::Forbidden);
		}
		if !aidash_domain::nodes::names_home(node, &run.home_node) {
			// A missing task and one the Run may not see read the same, as for a local delegation.
			let not_visible = || {
				Error::from(aidash_application::authorization::worker_tasks::task_not_visible(task))
			};
			let record = lease
				.delegation_task_read(task, &run.metadata())
				.await
				.map_err(|error| match error {
					Error::Forbidden => not_visible(),
					error => error,
				})?;
			if record.thread_id != run.thread_id {
				return Err(not_visible());
			}
			let identity = lease.identity.clone();
			if lease.tx.is_active() {
				lease.suspend().await?;
			}
			let repository = crate::apps::identity::repositories::home_execution::Repository {
				runtime: f.clone(),
				actor: Some(super::identity::Actor::Subject(identity)),
				worker: Some(run.metadata()),
				chain: None,
			};
			let result = aidash_application::authorization::home::delegate(
				&repository,
				task,
				node,
				agent,
				true,
			)
			.await
			.map_err(Into::into);
			// The delegation has reached the other Node, so a Run whose authority is gone
			// pauses; the model is not told the call was refused.
			refresh_access_for_run(&mut lease, &f.store, run)
				.await
				.map_err(authority_gone_after_effect)?;
			return result;
		}
		// Prepare the child's closure on the outer transaction: an inner shared
		// lock could queue behind a writer waiting for this same parent lease.
		// Admission below still checks the complete subject chain including the child.
		let entry = catalog::entry(&mut lease, agent, "agent.execute").await?;
		crate::marketplace::check_pinned(&mut lease, &entry).await?;
		crate::apps::registry::repositories::bindings::authorized_child(
			&mut lease,
			&entry,
			f.store.provider_credentials.is_some(),
		)
		.await?;
		let mut access = Access::under_lease(&lease).await?;
		let result = aidash_application::authorization::worker_tasks::delegate(
			&mut crate::bootstrap::worker_task_scope(f, &mut access),
			&run.metadata(),
			task,
			node,
			agent,
		)
		.await
		.map_err(Into::into);
		access.finish(result).await
	}
	pub async fn discover(&self, f: &Federation, search: &Search) -> Result<Discovery> {
		let mut access = self.access.write().await;
		let node = access.node.clone();
		super::peer::discovery::discover_in(f, &mut access, node, search).await
	}
}

pub(crate) use aidash_application::ports::execution::HumanMediaBatch;

pub(crate) struct Guard {
	pub(crate) scratch_space_id: Uuid,
	remote: Option<Federation>,
	access: Arc<RwLock<Access>>,
	run: Run,
	agent: AgentConfig,
	/// The Run's Tenant, learned from its authority when the step began.
	tenancy: aidash_application::tenancy::TenancyScope,
}

/// A Run's step as a worker begins it, for integration tests outside this
/// crate that call tools as the Run (#557). Production code holds the same
/// `Guard` inside the worker.
#[doc(hidden)]
pub struct Step(Guard);

impl Step {
	pub async fn begin(f: &Federation, run: &Run) -> Result<Self> {
		Guard::begin(f, run).await.map(Self)
	}

	/// The Run's Home carrying the step's authority, as the worker's tools see it.
	pub fn home(&self, f: Federation) -> crate::federation::Home {
		crate::federation::Home::new(f, self.0.run.clone())
			.with_authority(self.0.local_authority())
			.with_receiver_authority(self.0.receiver_authority())
	}

	/// Commits what the step wrote, as the worker does when it waits.
	pub async fn commit(&self) -> Result<()> {
		self.0.suspend().await
	}
}

pub(in crate::apps::identity) async fn authorize_guard(
	f: &Federation,
	run: &RunMetadata,
	access: &mut Access,
	read_context: bool,
) -> Result<AgentConfig> {
	let mark = access.call_mark();
	let result = aidash_application::authorization::execution::guard::authorize(
		&mut crate::bootstrap::run_guard_scope(f, access),
		run,
		read_context,
	)
	.await
	.map_err(Into::into);
	result.map_err(|error| access.named_since(mark, error))
}

struct SnapshotAuthority {
	remote: Option<Federation>,
	access: Arc<RwLock<Access>>,
	run: Run,
}
#[async_trait::async_trait]
impl aidash_application::ports::bindings::BindingAuthority for SnapshotAuthority {
	async fn refresh(&self, run: &Run) -> aidash_application::Result<()> {
		if run.id != self.run.id {
			return Err(aidash_application::Error::Forbidden);
		}

		aidash_application::authorization::tools::refresh(
			&crate::bootstrap::agent_tool_repository(self.remote.as_ref(), &self.access, run),
		)
		.await?;
		Ok(())
	}

	async fn check(
		&self,
		run: &Run,
		binding: &aidash_domain::registry::bindings::ResolvedBinding,
	) -> aidash_application::Result<()> {
		if run.id != self.run.id {
			return Err(aidash_application::Error::Forbidden);
		}
		let mut access = self.access.write().await;
		if binding.identity.registry_deployment != access.node_id {
			return Err(aidash_application::Error::Forbidden);
		}
		let current =
			catalog::entry(&mut access, &binding.identity.local(), "registry.read").await?;
		if aidash_domain::registry::rules::digest(&serde_json::to_value(&current)?)
			!= binding.digest
		{
			return Err(aidash_application::Error::Conflict(
				"admitted Binding definition changed".into(),
			));
		}
		crate::marketplace::check_pinned(&mut access, &current).await?;

		Ok(())
	}
	async fn check_metadata(
		&self,
		run: &Run,
		binding: &aidash_domain::registry::bindings::ResolvedBinding,
	) -> aidash_application::Result<()> {
		if run.id != self.run.id {
			return Err(aidash_application::Error::Forbidden);
		}
		let mut access = self.access.write().await;
		if binding.identity.registry_deployment != access.node_id {
			return Err(aidash_application::Error::Forbidden);
		}
		let current = catalog::metadata(&mut access, &binding.identity.local()).await?;
		if aidash_domain::registry::rules::digest(&serde_json::to_value(current)?) != binding.digest
		{
			return Err(aidash_application::Error::Conflict(
				"admitted Binding definition changed".into(),
			));
		}
		Ok(())
	}
}

impl Guard {
	pub(crate) fn binding_authority(
		&self,
	) -> Arc<dyn aidash_application::ports::bindings::BindingAuthority> {
		Arc::new(SnapshotAuthority {
			remote: self.remote.clone(),
			access: self.access.clone(),
			run: self.run.clone(),
		})
	}

	pub async fn begin(f: &Federation, run: &Run) -> Result<Self> {
		let entry = aidash_application::authorization::worker_entry::execution(
			&crate::bootstrap::worker_entry_repository(f),
			run,
		)
		.await?;
		Ok(Self {
			scratch_space_id: entry.scratch_space_id.ok_or(Error::Forbidden)?,
			tenancy: entry.scope.tenancy()?,
			remote: entry.remote.then(|| f.clone()),
			access: Arc::new(RwLock::new(entry.scope)),
			run: run.clone(),
			agent: entry.agent,
		})
	}
	pub async fn suspend(&self) -> Result<()> {
		let mut access = self.access.write().await;
		if access.tx.is_active() {
			access.suspend().await?;
		}
		Ok(())
	}
	pub async fn resume(&self, f: &Federation) -> Result<()> {
		aidash_application::authorization::resume::resume(
			&crate::bootstrap::worker_resume_repository(
				f,
				self.remote.as_ref(),
				&self.access,
				&self.run,
				&self.agent,
				self.scratch_space_id,
			),
		)
		.await
		.map_err(Into::into)
	}

	pub fn local_authority(&self) -> Option<WorkerAuthority> {
		self.remote.is_none().then(|| self.authority())
	}
	/// The authority of a Run executing for another Home, on this Node: the Peer Mapping's
	/// mapped subject within the Remote Grant (ADR 0139). `None` for a local Run, whose authority
	/// [`Self::local_authority`] already gives. Only core tools and Subnode operations take it;
	/// whatever acts on the Home's own Thread stays a Home command.
	pub fn receiver_authority(&self) -> Option<WorkerAuthority> {
		self.remote.is_some().then(|| self.authority())
	}
	pub fn is_remote(&self) -> bool {
		self.remote.is_some()
	}
	/// The scope of every transaction this Run's step opens.
	pub(crate) fn tenancy(&self) -> &aidash_application::tenancy::TenancyScope {
		&self.tenancy
	}
	/// The executing node's local Tenant of this worker: for a remote Run, the
	/// receiver's mapped Tenant, never the Home node's.
	pub(crate) async fn tenant(&self) -> String {
		self.access.write().await.identity.tenant.clone()
	}
	async fn refresh_remote(&self) -> Result<()> {
		aidash_application::authorization::tools::refresh(&crate::bootstrap::agent_tool_repository(
			self.remote.as_ref(),
			&self.access,
			&self.run,
		))
		.await
		.map_err(Into::into)
	}

	pub fn authority(&self) -> WorkerAuthority {
		WorkerAuthority {
			access: self.access.clone(),
			remote: self.remote.clone(),
		}
	}
	/// Retrieval Key scope: a local Run's subject and policy revision; a remote
	/// Run's admission values, without the Home's policy revision.
	pub(crate) async fn retrieval_scope(
		&self,
		store: &Store,
	) -> Result<aidash_domain::context::sources::RetrievalScope> {
		if let Some(federation) = &self.remote {
			let tenancy = self.access.read().await.tenancy()?;
			let description =
				super::peer::semantic::admission(federation, &self.run, &tenancy).await?;
			let (index_revision, participant_revision) = match &description.semantic {
				aidash_domain::semantic::remote::Binding::Disabled { .. } => (None, None),
				aidash_domain::semantic::remote::Binding::RequiredHome {
					index_revision,
					native,
					..
				} => (
					Some(*index_revision),
					native
						.as_ref()
						.map(|native| native.participant.participant_revision),
				),
			};
			// The Home's corpus is not observable here: its digest arrives with the
			// describe reply that refreshed this worker's authority, and is absent
			// from a Home that grants no semantic access.
			let corpus_digest = self.access.read().await.home_corpus_digest.clone();
			return Ok(aidash_domain::context::sources::RetrievalScope {
				tenant: description.source_tenant,
				subject: description.source_subject,
				authorization_revision: None,
				index_revision,
				participant_revision,
				corpus_digest,
			});
		}
		let (tenant, subject, authorization_revision) = {
			let access = self.access.write().await;
			(
				access.identity.tenant.clone(),
				access.identity.subject.clone(),
				access.snapshot.revision,
			)
		};
		let revisions =
			crate::semantic::services::memory_context::source_revisions(store, &self.run).await?;
		Ok(aidash_domain::context::sources::RetrievalScope {
			tenant,
			subject,
			authorization_revision: Some(authorization_revision),
			index_revision: revisions.index,
			participant_revision: revisions.participant,
			corpus_digest: Some(revisions.corpus),
		})
	}
	pub async fn semantic_context(
		&self,
		store: &Store,
		task: &Task,
		inputs: &[(crate::semantic::remote::InputRead, String)],
		budget: usize,
		key: Option<&aidash_domain::context::sources::RetrievalKey>,
		degrade_on_outage: bool,
	) -> aidash_application::ports::execution::SemanticContextRead {
		let mut thread_retrieved = false;
		let result: Result<Option<Value>> = async {
			let automatic = self
				.run
				.context
				.binding_snapshot
				.as_ref()
				.map(|snapshot| AgentConfig::automatic_context(snapshot))
				.transpose()?;
			let agent = automatic.as_ref().unwrap_or(&self.agent);
			let thread_budget = if self.remote.is_some() {
				budget
			} else {
				crate::semantic::services::memory_context::thread_budget(budget)?
			};
			let query = aidash_application::execution::semantic_context::query_material(
				&self.run,
				task,
				inputs,
				aidash_domain::context::source::MAX_SOURCE_BYTES,
			)?;
			let semantic = aidash_application::execution::semantic_context::retrieve_with_query(
				&crate::bootstrap::run_semantic_repository(
					store,
					self.remote.as_ref(),
					&self.access,
					&self.run,
					agent,
				),
				task,
				inputs,
				&query.text,
				thread_budget,
				// Required-Home semantic context never degrades (ADR 0059).
				degrade_on_outage && self.remote.is_none() && agent.thread_degrades,
			)
			.await
			.map_err(Error::from)?;
			thread_retrieved = semantic
				.as_ref()
				.is_some_and(|value| aidash_domain::semantic::degraded::degraded(value).is_none());
			if self.remote.is_some() {
				return Ok(semantic);
			}
			let reserved =
				serde_json::to_vec(&serde_json::json!({"thread":semantic,"memory":null}))?.len();
			let available = budget.saturating_sub(reserved);
			let mut access = self.access.write().await;
			let memory = crate::semantic::services::memory_context::retrieve(
				store,
				&mut crate::semantic::service::Lease::Inherited(&mut access),
				&self.run,
				task,
				inputs,
				available,
				agent,
				key,
			)
			.await?;
			crate::semantic::services::memory_context::complete(
				&mut crate::semantic::service::Lease::Inherited(&mut access),
				&self.run,
				semantic,
				memory,
				budget,
			)
			.await
		}
		.await;
		aidash_application::ports::execution::SemanticContextRead {
			selections: vec![],
			contributions: vec![],
			thread_retrieved,
			result: result.map_err(Into::into),
		}
	}
	pub(crate) async fn reference_image_context(
		&self,
		store: &Store,
		query: &str,
	) -> Result<Option<Value>> {
		if self.remote.is_some() {
			return Err(Error::Invalid(
				"reference images are private to their owning node".into(),
			));
		}
		let mut access = self.access.write().await;
		crate::apps::registry::services::knowledge::reference_image_context(
			&mut access,
			store,
			&self.run,
			aidash_domain::semantic::EmbeddingInput::Text(query),
			8,
		)
		.await
	}
	pub(crate) async fn recheck_source_observation(
		&self,
		store: &Store,
		content: &Value,
	) -> Result<()> {
		self.inference().await?;
		if self.agent.core_capabilities.skills {
			let deferred = self.agent.exposure_policy().is_deferred();
			let binding = self
				.run
				.context
				.binding_snapshot
				.as_ref()
				.ok_or_else(|| Error::Invalid("Run has no Binding snapshot".into()))?
				.operation(if deferred {
					aidash_domain::registry::bindings::SKILL_ASSET_READ
				} else {
					"skill_list"
				})?;
			let descriptor: aidash_domain::tool::providers::ToolDescriptor =
				serde_json::from_value(binding.definition.config.clone())?;
			let contract = descriptor.declared_contract(binding.identity.clone())?;
			self.tool_contract(
				&ToolCall {
					id: "source-observation".into(),
					name: binding.alias.clone().ok_or_else(|| {
						Error::Invalid("Skill support binding has no alias".into())
					})?,
					// Under deferred@1 an alias naming no Registry Skill takes the
					// direct-Skill authority path that pinned Skill context needs.
					arguments: if deferred {
						json!({"alias": ""})
					} else {
						json!({})
					},
				},
				&contract,
			)
			.await?;
		}
		if let Some(semantic) = content.get("semantic_memory").filter(|v| !v.is_null()) {
			if self.remote.is_some() {
				// Remote refresh verifies the retained source-disclosure journal on Home.
				aidash_application::authorization::tools::refresh(
					&crate::bootstrap::agent_tool_repository(
						self.remote.as_ref(),
						&self.access,
						&self.run,
					),
				)
				.await?;
			} else {
				let mut access = self.access.write().await;
				let mut lease = crate::semantic::service::Lease::Inherited(&mut access);
				crate::semantic::services::memory_context::recheck(
					store, &mut lease, &self.run, semantic,
				)
				.await?;
			}
		}
		Ok(())
	}

	pub async fn human_read(&self, id: Uuid) -> Result<()> {
		if let Some(remote) = &self.remote {
			let home = crate::federation::Home::new(remote.clone(), self.run.clone());
			let request = home.human_request_by_id(id).await?;
			if request.run_id != self.run.id || request.thread_id != self.run.thread_id {
				return Err(Error::Forbidden);
			}
			return Ok(());
		}

		aidash_application::authorization::tools::human_read(
			&crate::bootstrap::agent_tool_repository(self.remote.as_ref(), &self.access, &self.run),
			&self.run.metadata(),
			id,
		)
		.await
		.map_err(Into::into)
	}
	pub async fn action(&self, action: &str, kind: &str, id: impl ToString) -> Result<()> {
		aidash_application::authorization::tools::action(
			&crate::bootstrap::agent_tool_repository(self.remote.as_ref(), &self.access, &self.run),
			&self.run.metadata(),
			action,
			kind,
			&id.to_string(),
		)
		.await
		.map_err(Into::into)
	}

	pub async fn reserve_inference(
		&self,
		store: &Store,
		attempt: Uuid,
		window: usize,
		output: u32,
		request: &crate::provider::ModelRequest,
	) -> Result<Option<crate::generation::budget::InferenceReservation>> {
		aidash_application::execution::admission::reserve(
			&crate::bootstrap::inference_admission_repository(
				store,
				self.remote.as_ref(),
				&self.access,
				&self.run,
			),
			attempt,
			window,
			output,
			request,
		)
		.await
		.map_err(Into::into)
	}

	/// Whether every message is still readable by this Run under current
	/// authority. The recheck records no new read. Remote visibility belongs
	/// to the Home, so remote message dependencies fail closed.
	pub async fn messages_readable(&self, ids: &std::collections::BTreeSet<Uuid>) -> Result<bool> {
		if ids.is_empty() {
			return Ok(true);
		}
		if self.remote.is_some() {
			return Ok(false);
		}
		let mut access = self.access.write().await;
		aidash_application::authorization::records::messages_readable(
			&mut crate::bootstrap::run_visibility_scope(&mut access),
			self.run.thread_id,
			ids.iter().copied(),
		)
		.await
		.map_err(Into::into)
	}

	pub async fn inference(&self) -> Result<()> {
		self.refresh_remote().await?;
		self.inference_after_resume().await
	}

	pub(crate) async fn inference_after_resume(&self) -> Result<()> {
		let mut access = self.access.write().await;
		self.authorize_inference_with(&mut access).await
	}

	/// Resolve an explicit one-inference file selection under the
	/// same execution authority as the provider request. Only immutable
	/// references are durable.
	pub async fn model_media(
		&self,
		store: &Store,
		selections: &[aidash_domain::media::ModelMedia],
	) -> Result<Vec<crate::provider::ContentPart>> {
		aidash_application::execution::media::selected(
			&mut crate::bootstrap::selected_media(
				store,
				&self.run,
				self.access.clone(),
				self.remote
					.as_ref()
					.map(|f| crate::federation::Home::new(f.clone(), self.run.clone())),
			),
			selections,
		)
		.await
		.map_err(Into::into)
	}

	pub async fn human_message_media(
		&self,
		store: &Store,
		messages: &[(i64, Uuid, usize)],
		model: &crate::registry::ModelConfig,
	) -> Result<HumanMediaBatch> {
		let mut access = self.access.write().await;
		aidash_application::execution::media::authorized(
			&mut crate::bootstrap::scoped_media(&mut access, store),
			self.run.thread_id,
			messages,
			model,
		)
		.await
		.map_err(Into::into)
	}

	async fn authorize_inference_with(&self, access: &mut Access) -> Result<()> {
		aidash_application::authorization::inference::authorize(
			&mut crate::bootstrap::inference_approval_scope(
				access,
				&self.run,
				self.remote.is_some(),
			),
			&self.run.metadata(),
			&self.agent,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn subnode_tool_filter(
		&self,
		tools: &mut aidash_application::ports::execution::Tools,
	) -> Result<bool> {
		aidash_application::authorization::tools::filter_subnode(
			&crate::bootstrap::agent_tool_repository(self.remote.as_ref(), &self.access, &self.run),
			tools,
			|tool| tool.contract(),
		)
		.await
		.map_err(Into::into)
	}

	/// Drop the tools a remote Run may never see (ADR 0139); a Run of this Tenant keeps its tools.
	pub(crate) async fn remote_tool_filter(
		&self,
		tools: &mut aidash_application::ports::execution::Tools,
	) -> Result<()> {
		aidash_application::authorization::tools::filter_remote(
			&crate::bootstrap::agent_tool_repository(self.remote.as_ref(), &self.access, &self.run),
			tools,
			|tool| tool.contract(),
		)
		.await
		.map_err(Into::into)
	}

	pub async fn tool_contract(
		&self,
		call: &ToolCall,
		contract: &aidash_domain::tool::ToolContract,
	) -> Result<()> {
		aidash_application::authorization::tools::authorize(
			&crate::bootstrap::agent_tool_repository(self.remote.as_ref(), &self.access, &self.run),
			&self.run.metadata(),
			&self.agent,
			call,
			contract,
		)
		.await
		.map_err(Into::into)
	}

	pub async fn finish(self, result: Result<()>) -> Result<()> {
		let access = Arc::try_unwrap(self.access)
			.map_err(|_| Error::Conflict("execution boundary still in use".into()))?
			.into_inner();
		access.finish(result).await
	}
}

pub async fn resume(f: &Federation, identity: &SubjectIdentity, id: Uuid) -> Result<RunInspection> {
	aidash_application::authorization::runs::resume(
		&crate::bootstrap::run_control_repository(f, identity),
		id,
	)
	.await
	.map_err(Into::into)
}

/// The Runs `identity` started that an Account suspension paused, in Run
/// order, for its owner to resume them together (#876 decision 17).
pub async fn suspension_paused(f: &Federation, identity: &SubjectIdentity) -> Result<Vec<Uuid>> {
	use crate::apps::execution::models::Run;
	use crate::apps::execution::services::states::RunControl;
	use crate::apps::identity::models::AuthorizationExecution;
	use reinhardt::db::orm::{Model, QuerySet};
	let scope = aidash_application::tenancy::TenancyScope::tenant(&identity.tenant)?;
	let mut tx = f.store.pool.begin(&scope).await?.into_executor().await?;
	let grants = AuthorizationExecution::objects()
		.filter(AuthorizationExecution::field_tenant().eq(identity.tenant.clone()))
		.filter(AuthorizationExecution::field_root_subject().eq(identity.subject.clone()))
		.filter_in_subquery("run_id", |runs: QuerySet<Run>| {
			runs.filter(Run::field_control().eq(RunControl::Paused))
				.filter(Run::field_error().eq(Some("account_suspended".to_owned())))
				.values(&["id"])
		})?
		.order_by(&["run_id"])
		.all_with_executor(tx.as_mut())
		.await
		.map_err(reinhardt::core::exception::Error::from)?;
	tx.commit().await?;
	Ok(grants.into_iter().map(|grant| grant.run_id).collect())
}

pub async fn decline_call(
	f: &Federation,
	identity: &SubjectIdentity,
	id: Uuid,
	call: &str,
	rest_of_run: bool,
) -> Result<RunInspection> {
	aidash_application::authorization::runs::decline_call(
		&crate::bootstrap::run_control_repository(f, identity),
		id,
		call,
		rest_of_run,
	)
	.await
	.map_err(Into::into)
}

pub async fn details(f: &Federation, identity: &SubjectIdentity, id: Uuid) -> Result<RunDetails> {
	details_page(f, identity, id, 0).await
}
pub async fn details_page(
	f: &Federation,
	identity: &SubjectIdentity,
	id: Uuid,
	offset: u64,
) -> Result<RunDetails> {
	let details = aidash_application::authorization::run_details::inspect(
		&crate::bootstrap::run_details_repository(f, identity),
		id,
		offset,
	)
	.await?;
	Ok(RunDetails {
		provider_tool_activity: details.provider_tool_activity,
		egress_requests: details.egress_requests,
		egress_waits: details.egress_waits,
		stints: details.stints,
		run: details.run,
		declinable_call: details.declinable_call,
		invocations: details.invocations.into_iter().map(Into::into).collect(),
		memory: details.memory,
		media_input_routes: details.media_input_routes,
	})
}

pub async fn discover(
	f: &Federation,
	identity: &SubjectIdentity,
	node: Option<aidash_domain::nodes::NodeName>,
	search: &Search,
) -> Result<Discovery> {
	super::peer::discovery::discover(f, identity, node, search).await
}

pub(crate) async fn cancel_if_scoped(store: &Store, run: &Run, token: Uuid) -> Result<bool> {
	aidash_application::execution::cancellation::cancel_if_scoped(
		&crate::bootstrap::scoped_cancellation(store),
		run,
		token,
	)
	.await
	.map_err(Into::into)
}

pub(crate) struct DeliveryGuard {
	access: Arc<RwLock<Access>>,
	remote: bool,
}

impl DeliveryGuard {
	pub async fn begin(f: &Federation, run: &RunMetadata) -> Result<Self> {
		let entry = aidash_application::authorization::worker_entry::delivery(
			&crate::bootstrap::worker_entry_repository(f),
			run,
		)
		.await?;
		Ok(Self {
			access: Arc::new(RwLock::new(entry.scope)),
			remote: entry.remote,
		})
	}
}

impl DeliveryGuard {
	pub fn local_authority(&self) -> Option<WorkerAuthority> {
		(!self.remote).then(|| WorkerAuthority {
			access: self.access.clone(),
			remote: None,
		})
	}
}

impl DeliveryGuard {
	pub async fn finish<T>(self, result: Result<T>) -> Result<T> {
		Arc::try_unwrap(self.access)
			.map_err(|_| Error::Conflict("delivery boundary still in use".into()))?
			.into_inner()
			.finish(result)
			.await
	}
}
#[path = "execution/management.rs"]
pub(crate) mod management;
#[path = "execution/safety_stop.rs"]
pub(crate) mod safety_stop;

#[path = "execution/plugin_stdio.rs"]
mod plugin_stdio;
#[path = "execution/subnodes.rs"]
pub(crate) mod subnodes;

impl WorkerAuthority {
	pub(crate) async fn completion_attachments(
		&self,
		store: &Store,
		run: &Run,
		home: &Home,
		message: Uuid,
		paths: &[String],
	) -> Result<Vec<Uuid>> {
		use crate::apps::execution::repositories::{deliverables, operations::pods};
		use reinhardt::query::{
			Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
		};

		let mut outer = self.access.write().await;
		if home.local() {
			let thread = outer.thread(run.thread_id).await?;
			outer.require(&thread, "message.create").await?;
		}
		let tenant = outer.identity.tenant.clone();
		let pod = pods::tool_context(store, &mut outer, &run.metadata())
			.await?
			.ok_or_else(|| Error::Conflict("Scratch Space Execution Pod is unavailable".into()))?;
		let ((staged, uploads), _visibility) = self
			.without_pod_file_authority(&mut outer, store, run, async {
				deliverables::stage_attachments(store, &tenant, &pod, message, paths)
					.await
					.map_err(Into::into)
			})
			.await?;
		if !home.local() {
			drop(outer);
			let ids = home
				.deliver_completion_attachments(&tenant, message, &staged)
				.await?;
			uploads.commit().await?;
			return Ok(ids);
		}
		let mut access = Access::under_lease(&outer).await?;
		let result = async {
			let thread = access.thread(run.thread_id).await?;
			access.require(&thread, "message.create").await?;
			let current = pods::settle_file_context(store, &mut access, &run.metadata())
				.await?
				.ok_or(Error::Forbidden)?;
			if pod.scratch_space_id() != current.scratch_space_id() {
				return Err(Error::Conflict("Task Scratch Space changed".into()));
			}
			let mut ids = Vec::with_capacity(staged.len());
			let sender = run.executor_subject(store.deployment.as_str());
			for attachment in staged {
				let exists: Option<Uuid> = crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("id"))
						.from(Alias::new("channel_attachments"))
						.and_where(Expr::col("id").eq(Expr::value(attachment.id)))
						.and_where(Expr::col("thread_id").eq(Expr::value(run.thread_id)))
						.and_where(Expr::col("uploaded_by").eq(sender.clone()))
						.to_string(PostgresQueryBuilder),
				)
				.scalar_optional(&mut **access.tx)
				.await?;
				if exists.is_none() {
					let (object, digest) = attachment.object.publish(&mut access).await?;
					if digest != attachment.digest {
						return Err(Error::Conflict("Attachment digest changed".into()));
					}
					crate::apps::workspaces::models::ChannelAttachment::stage_completion(
						&mut **access.tx,
						crate::apps::workspaces::models::channel_attachments::CompletionAttachment {
							id: attachment.id, thread: run.thread_id, sender: &sender, message,
							filename: &attachment.filename, media_type: &attachment.media_type,
							digest: &digest, size: attachment.size, object,
						}
					).await?;
				}
				ids.push(attachment.id);
			}
			Ok(ids)
		}
		.await;
		let ids = access.finish(result).await?;
		uploads.commit().await?;
		Ok(ids)
	}
}
