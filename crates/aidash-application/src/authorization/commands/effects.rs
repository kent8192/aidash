//! Scoped effects preserve authorization order, durable replay and the final lease check.
use super::{Admission, Command, Prepared, prepare};
use crate::{
	Error, Result,
	ports::authorization::commands::effects::{
		Applied, Effect, RemoteCommandEffects, WriteContext,
	},
};
use aidash_domain::{
	NewTask, TaskStatus,
	federation::Delegation,
	identity::commands::{
		self, Metadata,
		effects::{AuditError, audit_event, child_eligible, field},
	},
	policy::Resource,
	registry::{EntityRef, Entry, Search},
};
use serde_json::{Value, json};
use uuid::Uuid;
pub async fn execute(
	scope: &mut dyn RemoteCommandEffects,
	input: Command<'_>,
	agent: &Entry,
) -> Result<Value> {
	let command = Command {
		grant_id: input.grant_id,
		admission_id: input.admission_id,
		operation: input.operation,
		data: input.data,
		node: input.node,
		agent: input.agent,
		version: input.version,
		request: input.request,
	};
	if matches!(
		input.operation,
		"human_request"
			| "connection_request"
			| "outbound_request"
			| "mcp.connection_call"
			| "mcp.connection_list"
	) {
		let bound = scope
			.binding(input.grant_id)
			.await?
			.ok_or(Error::Forbidden)?;
		if bound.admission_id != input.admission_id {
			return Err(Error::Forbidden);
		}
		let task = scope.task(bound.task_id).await?;
		let attributes = scope.task_resource(&task).await?.attributes;
		let resource = scope.resource(
			"run",
			&format!("{}/runs/{}", scope.local_node(), input.admission_id),
			attributes,
		);
		scope
			.require(
				&resource,
				if matches!(
					input.operation,
					"human_request" | "connection_request" | "outbound_request"
				) {
					"human.request"
				} else {
					"tool.invoke"
				},
			)
			.await?;
	}
	let connection_call = if matches!(
		input.operation,
		"mcp.connection_call" | "mcp.connection_list"
	) {
		let call: aidash_domain::identity::commands::connections::ConnectionCall =
			serde_json::from_value(input.data.clone())?;
		call.validate(input.operation == aidash_domain::identity::commands::connections::LIST)?;
		scope
			.connection_target(
				&call,
				input.operation == aidash_domain::identity::commands::connections::LIST,
			)
			.await?;
		Some(call)
	} else {
		None
	};
	let connection_owner = connection_call
		.as_ref()
		.and_then(|_| scope.connection_owner());
	if let Some(call) = &connection_call
		&& connection_owner.is_none()
	{
		let result = scope
			.connection_unowned(
				call,
				input.operation == aidash_domain::identity::commands::connections::LIST,
			)
			.await?;
		if !scope.live(input.grant_id).await? {
			return Err(Error::Forbidden);
		}
		return Ok(result);
	}

	let Prepared {
		task,
		owner,
		metadata,
		key,
		task_resource,
	} = match prepare(scope, command).await? {
		Admission::Replay(result) => return Ok(result),
		Admission::Ready(prepared) => *prepared,
	};
	let Metadata {
		mutation,
		digest,
		request_key,
	} = metadata;
	let data = input.data;
	let context = || WriteContext {
		task: &task,
		owner: &owner,
		key: &key,
		admission: input.admission_id,
	};
	let result = match input.operation {
		"mcp.connection_call" | "mcp.connection_list" => {
			let call = connection_call.as_ref().ok_or(Error::Forbidden)?;
			let list = input.operation == aidash_domain::identity::commands::connections::LIST;
			scope
				.apply(
					context(),
					Effect::Connection {
						owner: connection_owner.as_ref().ok_or(Error::Forbidden)?,
						call,
						list,
					},
				)
				.await?
				.value
		}
		"human_request" => {
			let run = scope.resource(
				"run",
				&format!("{}/runs/{}", scope.local_node(), input.admission_id),
				task_resource.attributes.clone(),
			);
			scope.require(&run, "human.request").await?;
			scope
				.apply(
					context(),
					Effect::HumanRequest {
						kind: field(data, "kind")?,
						prompt: field(data, "prompt")?,
					},
				)
				.await?
				.value
		}
		"connection_request" => {
			only_fields(data, &["run_id", "key", "prompt"])?;
			// The Home's person at the root of the chain is the one asked to connect.
			if scope.connection_owner().is_none() {
				return Err(Error::Invalid(
					"connection_request requires a person at the Run's chain root".into(),
				));
			}
			let run = scope.resource(
				"run",
				&format!("{}/runs/{}", scope.local_node(), input.admission_id),
				task_resource.attributes.clone(),
			);
			scope.require(&run, "human.request").await?;
			let prompt = field(data, "prompt")?;
			require_connection_prompt(prompt)?;
			scope
				.apply(
					context(),
					Effect::HumanRequest {
						kind: aidash_domain::oauth_connections::CONNECTION_REQUEST_KIND,
						prompt,
					},
				)
				.await?
				.value
		}
		"outbound_request" => {
			only_fields(data, &["run_id", "key", "prompt"])?;
			// The Home's person at the root of the chain is the one who may allow the fetch.
			if scope.connection_owner().is_none() {
				return Err(Error::Invalid(
					"outbound_get requires a person at the Run's chain root".into(),
				));
			}
			let run = scope.resource(
				"run",
				&format!("{}/runs/{}", scope.local_node(), input.admission_id),
				task_resource.attributes.clone(),
			);
			scope.require(&run, "human.request").await?;
			let prompt = field(data, "prompt")?;
			aidash_domain::capabilities::approvals::check_outbound_prompt(prompt)?;
			scope
				.apply(
					context(),
					Effect::HumanRequest {
						kind: aidash_domain::capabilities::approvals::OUTBOUND_REQUEST_KIND,
						prompt,
					},
				)
				.await?
				.value
		}
		"outbound_read" => {
			only_fields(data, &["run_id", "id"])?;
			let request = scope
				.human_read(
					input.grant_id,
					input.admission_id,
					serde_json::from_value(data["id"].clone())?,
				)
				.await?;
			// Only the question this tool put to the person can be read through it: an
			// approval request whose prompt names an outbound GET, never another tool's.
			if request.kind != aidash_domain::capabilities::approvals::OUTBOUND_REQUEST_KIND
				|| aidash_domain::capabilities::approvals::check_outbound_prompt(&request.prompt)
					.is_err()
			{
				return Err(Error::Forbidden);
			}
			json!(request)
		}
		"thread_search" => {
			only_fields(data, &["run_id", "input"])?;
			let resource = scope.thread(task.thread_id).await?;
			scope.require(&resource, "thread.read").await?;
			// An image query names a File or a Thread Attachment selection of the
			// receiving Node's Run, which the Home cannot read.
			if data["input"]
				.get("image")
				.is_some_and(|image| !image.is_null())
			{
				return Err(Error::Invalid(
					"a Run executing for another Home searches this Thread by text only".into(),
				));
			}
			scope.thread_search(task.thread_id, &data["input"]).await?
		}
		"task_assign" => {
			only_fields(data, &["run_id", "key", "task_id", "policy", "reason"])?;
			let id: Uuid = serde_json::from_value(data["task_id"].clone())?;
			let child = scope.task(id).await?;
			// Only a child this grant created, as for a delegation. Who may be assigned is
			// decided by the Home's Generation policy for the delegation chain.
			if !child_eligible(&task, &child, &owner)
				|| !scope.created_by_grant(input.grant_id, child.id).await?
			{
				return Err(Error::Forbidden);
			}
			scope
				.apply(
					context(),
					Effect::Assign {
						child: child.id,
						policy: field(data, "policy")?,
						reason: field(data, "reason")?,
					},
				)
				.await?
				.value
		}
		"memory_recall" | "memory_reflect" => {
			only_fields(data, &["run_id", "key", "query"])?;
			let query: aidash_domain::memory::RecallQuery =
				serde_json::from_value(data["query"].clone())?;
			scope
				.memory_recall(
					input.grant_id,
					input.admission_id,
					field(data, "key")?,
					&query,
					input.operation == "memory_reflect",
				)
				.await?
		}
		"memory_mutate" => {
			only_fields(data, &["run_id", "key", "changes"])?;
			let changes: Vec<aidash_domain::memory::Change> =
				serde_json::from_value(data["changes"].clone())?;
			scope
				.memory_mutate(
					input.grant_id,
					input.admission_id,
					field(data, "key")?,
					&changes,
				)
				.await?
		}
		"human_read" => json!(
			scope
				.human_read(
					input.grant_id,
					input.admission_id,
					serde_json::from_value(data["id"].clone())?
				)
				.await?
		),
		"task" => json!(task),
		"snapshot" => scope.snapshot(task.thread_id).await?,
		"workspace_observation" => {
			let offset = serde_json::from_value(data["offset"].clone())?;
			let limit: usize = serde_json::from_value(data["limit"].clone())?;
			if !(1..=aidash_domain::context::observation::MAX_LIMIT).contains(&limit) {
				return Err(Error::Invalid("invalid thread observation limit".into()));
			}
			// A per-step view names its own Run to leave out (#942); older
			// callers send no key and get every event.
			let exclude_run: Option<Uuid> = match data.get("exclude_run") {
				None | Some(Value::Null) => None,
				Some(value) => Some(serde_json::from_value(value.clone())?),
			};
			scope
				.observation(task.thread_id, offset, limit, exclude_run)
				.await?
		}
		"thread_record" | "thread_record_chunk" => {
			let id: Uuid = serde_json::from_value(data["id"].clone())?;
			let kind = field(data, "kind")?;
			let record = scope.record(task.thread_id, kind, id).await?;
			if input.operation == "thread_record_chunk" {
				let offset = serde_json::from_value(data["offset"].clone())?;
				let max_chars: usize = serde_json::from_value(data["max_chars"].clone())?;
				if max_chars > 16000 {
					return Err(Error::Invalid(
						"thread chunk exceeds 16000 characters".into(),
					));
				}
				aidash_domain::context::observation::chunk_record(
					record,
					kind,
					&id.to_string(),
					offset,
					max_chars,
				)?
			} else {
				record
			}
		}
		// Whether a Task is one this grant created, so the receiver may let its
		// Run hand it on (#654). Reads nothing else about the Task.
		"created_task" => {
			let id: Uuid = serde_json::from_value(data["task_id"].clone())?;
			json!({"created": scope.created_by_grant(input.grant_id, id).await?})
		}
		"discover" => {
			let search: Search = match data.get("search") {
				None | Some(Value::Null) => Search::default(),
				Some(search) => serde_json::from_value(search.clone())?,
			};
			serde_json::to_value(scope.discover(task.thread_id, &search).await?)?
		}
		"thread_children" => {
			let parent: Uuid = serde_json::from_value(data["parent_id"].clone())?;
			if parent != task.id {
				return Err(Error::Forbidden);
			}
			scope.children(task.thread_id, parent).await?
		}
		"claim" => {
			scope.require(&task_resource, "task.execute").await?;
			if data["entry"] != json!(agent) {
				return Err(Error::Forbidden);
			}
			scope
				.apply(
					context(),
					Effect::Claim {
						revision: serde_json::from_value(data["revision"].clone())?,
						agent,
					},
				)
				.await?
				.value
		}
		"transition" | "run_message_terminal_transition" => {
			scope.require(&task_resource, "task.execute").await?;
			let revision = serde_json::from_value(data["revision"].clone())?;
			let next: TaskStatus = serde_json::from_value(data["status"].clone())?;
			let reason = match data.get("reason") {
				None => None,
				Some(value)
					if input.operation == "run_message_terminal_transition"
						&& next == TaskStatus::Failed
						&& value == aidash_domain::capabilities::sessions::QUEUE_LIMIT =>
				{
					Some(aidash_domain::capabilities::sessions::QUEUE_LIMIT)
				}
				Some(_) => return Err(Error::Invalid("invalid remote failure reason".into())),
			};
			let through = if input.operation == "run_message_terminal_transition" {
				let seq: i64 = serde_json::from_value(data["through_seq"].clone())?;
				if seq < 0 {
					return Err(Error::Invalid("invalid terminal sequence".into()));
				}
				Some(seq)
			} else {
				None
			};
			scope
				.apply(
					context(),
					Effect::Transition {
						revision,
						next,
						through,
						reason,
					},
				)
				.await?
				.value
		}
		"child_goal" => {
			let id: Uuid = serde_json::from_value(data["child"].clone())?;
			let child = scope.task(id).await?;
			if child.parent_id != Some(task.id)
				|| child.thread_id != task.thread_id
				|| child.created_by != owner
				|| !scope.created_by_grant(input.grant_id, child.id).await?
			{
				return Err(Error::Forbidden);
			}
			let resource = scope.task_resource(&child).await?;
			scope.require(&resource, "task.delegate").await?;
			scope
				.apply(
					context(),
					Effect::ChildGoal {
						child: id,
						goal: field(data, "goal")?,
					},
				)
				.await?
				.value
		}
		"goal_verdict" => {
			scope.require(&task_resource, "task.complete").await?;
			let verdict: aidash_domain::goal::VerdictRecord =
				serde_json::from_value(data["verdict"].clone())?;
			if verdict.run_id != input.admission_id
				|| task.goal.as_deref() != Some(verdict.goal.as_str())
			{
				return Err(Error::Conflict(
					"Verdict belongs to another Goal or execution".into(),
				));
			}
			scope
				.apply(context(), Effect::GoalVerdict { verdict: &verdict })
				.await?
				.value
		}
		"run_message_complete" => {
			if data["settlement_version"] != 2 {
				return Err(Error::Invalid(
					"Unsupported completion settlement version".into(),
				));
			}
			scope.require(&task_resource, "task.complete").await?;
			let completion: aidash_domain::deliverable::Completion =
				serde_json::from_value(data["completion"].clone())?;
			let through: i64 = serde_json::from_value(data["through_seq"].clone())?;
			if through < 0
				|| completion.message_id
					!= aidash_domain::deliverable::Completion::message_id(
						task.id,
						field(data, "key")?,
					) {
				return Err(Error::Invalid(
					"Invalid completion settlement identity".into(),
				));
			}
			let abandon = abandon_list(data)?;
			require_abandonment(scope, &task, &abandon).await?;
			let result = scope
				.apply(
					context(),
					Effect::Complete {
						completion: &completion,
						through,
						abandon: &abandon,
					},
				)
				.await?;
			track(scope, &result, input.grant_id, task.thread_id, "message").await?;
			result.value
		}
		"run_abandon_unassigned" => {
			let ids = abandon_list(data)?;
			if ids.is_empty() {
				return Err(Error::Invalid("no tasks to abandon".into()));
			}
			require_abandonment(scope, &task, &ids).await?;
			scope
				.apply(context(), Effect::AbandonUnassigned { ids: &ids })
				.await?
				.value
		}
		"create_task" => {
			let resource = scope.thread(task.thread_id).await?;
			scope.require(&resource, "task.create").await?;
			let new: NewTask = serde_json::from_value(data["task"].clone())?;
			if new.parent_id != Some(task.id) {
				return Err(Error::Forbidden);
			}
			for id in &new.dependencies {
				let dependency = scope.task(*id).await?;
				if dependency.thread_id != task.thread_id {
					return Err(Error::Forbidden);
				}
			}
			let result = scope
				.apply(context(), Effect::CreateTask { task: &new })
				.await?;
			track(scope, &result, input.grant_id, task.thread_id, "task").await?;
			result.value
		}
		"delegate" => {
			let id: Uuid = serde_json::from_value(data["task_id"].clone())?;
			let child = scope.task(id).await?;
			if !child_eligible(&task, &child, &owner) {
				return Err(Error::Forbidden);
			}
			let created = scope.created_by_grant(input.grant_id, child.id).await?;
			// Another Node is reached through [`onward`] and a Remote Grant of the
			// Home's own, never inside this lease.
			if !created || field(data, "node_id")? != scope.local_node() {
				return Err(Error::Forbidden);
			}
			let agent: EntityRef = serde_json::from_value(data["agent"].clone())?;
			scope
				.apply(
					context(),
					Effect::Delegate {
						child: child.id,
						agent: &agent,
					},
				)
				.await?
				.value
		}
		"message" | "run_message_output" => {
			let resource = scope.thread(task.thread_id).await?;
			scope.require(&resource, "message.create").await?;
			let content = field(data, "content")?;
			let through = if input.operation == "run_message_output" {
				Some(serde_json::from_value(data["included_input_seq"].clone())?)
			} else {
				None
			};
			let result = scope
				.apply(context(), Effect::Message { content, through })
				.await?;
			track(scope, &result, input.grant_id, task.thread_id, "message").await?;
			json!({"sent":true})
		}
		"run_message_delivery_capability" => json!({"protocol":2}),
		"run_message_reserve" | "run_message_commit" | "run_message_delivery" => {
			let run = scope.resource(
				"run",
				&input.admission_id.to_string(),
				task_resource.attributes.clone(),
			);
			scope.require(&run, "run.message").await?;
			let resource = scope.thread(task.thread_id).await?;
			scope.require(&resource, "message.create").await?;
			let key = field(data, "key")?;
			commands::require_input_key(key, input.admission_id)?;
			let content = field(data, "content")?;
			aidash_domain::nonempty(content, "run message")?;
			let effect = match input.operation {
				"run_message_reserve" => Effect::ReserveInput {
					node: input.node,
					key,
					content,
				},
				"run_message_commit" => Effect::CommitInput {
					key,
					content,
					seq: serde_json::from_value(data["input_seq"].clone())?,
				},
				_ => Effect::DeliverInput {
					node: input.node,
					key,
					content,
				},
			};
			scope.apply(context(), effect).await?.value
		}
		"run_message_release" | "run_message_ack" => {
			let keys: Vec<String> = serde_json::from_value(data["keys"].clone())?;
			if keys.len() > 1024 {
				return Err(Error::Invalid("too many input keys".into()));
			}
			for key in &keys {
				commands::require_input_key(key, input.admission_id)?;
			}
			if input.operation == "run_message_release" {
				scope
					.apply(
						context(),
						Effect::ReleaseInputs {
							node: input.node,
							keys: &keys,
						},
					)
					.await?;
			}
			json!({"acknowledged":true})
		}
		"run_queued" => {
			scope.require(&task_resource, "task.execute").await?;
			let queued: aidash_domain::federation::execution::admission::RunQueued =
				serde_json::from_value(data.clone())?;
			if queued.task_id != task.id || queued.run_id != input.admission_id {
				return Err(Error::Forbidden);
			}
			if !(1..100).contains(&queued.runs_ahead) {
				return Err(Error::Invalid("invalid Run queue position".into()));
			}
			scope
				.apply(
					context(),
					Effect::Event {
						kind: "remote_run.queued",
						data: json!({
							"task_id": task.id,
							"remote_run_id": input.admission_id,
							"runs_ahead": queued.runs_ahead,
							"queued_at": queued.queued_at,
						}),
					},
				)
				.await?
				.value
		}
		"event" => {
			let (kind, detail) =
				audit_event(data, input.admission_id).map_err(|error| match error {
					AuditError::Unbound => Error::Forbidden,
					AuditError::Invalid(error) => error.into(),
				})?;
			scope.apply(context(), Effect::Event {kind, data:json!({"task_id":task.id,"grant_id":input.grant_id,"remote_run_id":input.admission_id,"detail":detail})}).await?;
			json!({"recorded":true})
		}
		"run_message_history" => {
			let offset: usize = serde_json::from_value(data["offset"].clone())?;
			let rows = scope
				.history(task.thread_id, task.id, input.node, offset)
				.await?;
			let mut visible = Vec::new();
			for message in rows {
				visible.push(scope.record(task.thread_id, "message", message.id).await?);
			}
			json!(visible)
		}
		_ => return Err(Error::Invalid("unsupported scoped home operation".into())),
	};
	if mutation
		&& !(input.operation == aidash_domain::identity::commands::connections::CALL
			&& aidash_domain::identity::commands::connections::retryable(&result))
	{
		scope
			.persist_replay(input.grant_id, task.id, &request_key, &digest, &result)
			.await?;
	}
	if !scope.live(input.grant_id).await? {
		return Err(Error::Forbidden);
	}
	Ok(result)
}
/// A scoped `delegate` that names a Node other than the Home's own (#654,
/// ADR 0109). The Home hands the child on through a Remote Grant of its own,
/// which it prepares and activates only after the command's lease ends: both
/// talk to the target Node, and the Home releases its locks before peer calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Onward {
	pub child: Uuid,
	pub node: String,
	pub agent: EntityRef,
	/// Where the answer is saved once the child is handed on.
	pub replay: OnwardReplay,
}

/// The replay row of an onward `delegate`, as `execute` saves one for a local
/// delegation: under the asking grant, the command's key and the digest of its
/// input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnwardReplay {
	pub request_key: String,
	pub digest: String,
}

/// What [`onward`] decided under the command's lease.
#[derive(Debug, Clone, PartialEq)]
pub enum OnwardStep {
	/// The same command already handed the child on: its saved answer.
	Replay(Value),
	/// Hand the child on with [`hand_on`] once the lease has ended.
	HandOn(Onward),
}

/// Whether a scoped command hands a child Task to another Node.
pub fn is_onward(local_node: &str, operation: &str, data: &Value) -> bool {
	operation == "delegate"
		&& data
			.get("node_id")
			.and_then(Value::as_str)
			.is_some_and(|node| node != local_node)
}

/// Checks an onward `delegate` under the command's lease as strictly as one
/// for the Home's own Node: a live binding, no conflicting replay, a child of
/// the Run's Task that this grant created and the caller may hand on. It
/// prepares nothing; the caller hands the child on after the lease ends, and
/// the grant it prepares re-checks the whole chain by the Home's policy.
///
/// A command the Home already handed on answers what it answered then. A
/// harness re-executes a replay-safe call under its key after losing its
/// lease, and by then the target may have claimed the child, which a fresh
/// check would refuse for a hand-on that happened.
pub async fn onward(
	scope: &mut dyn RemoteCommandEffects,
	input: Command<'_>,
) -> Result<OnwardStep> {
	let data = input.data;
	let grant = input.grant_id;
	if !is_onward(scope.local_node(), input.operation, data) {
		return Err(Error::Invalid(
			"only a delegation to another Node is handed on".into(),
		));
	}
	let node = field(data, "node_id")?.to_owned();
	aidash_domain::nodes::NodeAddress::parse(&node)?;
	let Prepared {
		task,
		owner,
		metadata,
		..
	} = match prepare(scope, input).await? {
		// `prepare` refused a different input under the same key.
		Admission::Replay(result) => return Ok(OnwardStep::Replay(result)),
		Admission::Ready(prepared) => *prepared,
	};
	let id: Uuid = serde_json::from_value(data["task_id"].clone())?;
	let child = scope.task(id).await?;
	if !child_eligible(&task, &child, &owner) || !scope.created_by_grant(grant, child.id).await? {
		return Err(Error::Forbidden);
	}
	let agent: EntityRef = serde_json::from_value(data["agent"].clone())?;
	if !scope.live(grant).await? {
		return Err(Error::Forbidden);
	}
	Ok(OnwardStep::HandOn(Onward {
		child: child.id,
		node,
		agent,
		replay: OnwardReplay {
			request_key: metadata.request_key,
			digest: metadata.digest,
		},
	}))
}

/// The Home's side of [`hand_on`]. Every step takes its own short transaction,
/// because handing on talks to the target Node.
#[async_trait::async_trait]
pub trait HandOnRepository: Send + Sync {
	type Scope: HandOnScope;
	/// A transaction of the Home as the chain's root person, deciding for every
	/// subject of `chain`.
	async fn begin(&self, chain: &[String]) -> Result<Self::Scope>;
	/// Prepares, or reuses, and activates this Home's Remote Grant of the child
	/// for exactly `chain`.
	async fn delegate(&self, chain: Vec<String>, onward: &Onward) -> Result<Delegation>;
	/// Saves `result` as the answer to a repeat of the asking grant's command.
	async fn save_replay(&self, grant: Uuid, replay: &OnwardReplay, result: &Value) -> Result<()>;
}

/// One transaction of [`HandOnRepository::begin`].
#[async_trait::async_trait]
pub trait HandOnScope: Send + Sized {
	/// Whether `grant` still lives by the Home's one liveness rule.
	async fn live(&mut self, grant: Uuid) -> Result<bool>;
	async fn task_read(&mut self, id: Uuid) -> Result<aidash_domain::Task>;
	async fn task_resource(&mut self, task: &aidash_domain::Task) -> Result<Resource>;
	fn node_resource(&self, node: &str) -> Resource;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	/// Commits, or records the refusal and rolls back.
	async fn finish(self, result: Result<()>) -> Result<()>;
}

/// Hands the child that [`onward`] checked on to its Node through a Remote
/// Grant of this Home (#654, ADR 0109), under the asking grant's whole chain,
/// never as its root person alone. The asking grant must still live: it may
/// have been revoked since the command's lease ended. Every subject of the
/// chain, the asking receiver's executor included, must be allowed
/// `task.delegate` on the child and `federation.execute` on the target Node;
/// the grant it prepares carries that chain, and the target decides by its own
/// consent. The answer is saved for a repeat of the command.
pub async fn hand_on<R: HandOnRepository>(
	repository: &R,
	root: &str,
	grant: Uuid,
	chain: Vec<String>,
	onward: Onward,
) -> Result<Value> {
	if chain.len() < 2 || chain.first().map(String::as_str) != Some(root) {
		return Err(Error::Forbidden);
	}
	let mut scope = repository.begin(&chain).await?;
	let checked = async {
		if !scope.live(grant).await? {
			return Err(Error::Forbidden);
		}
		let child = scope.task_read(onward.child).await?;
		let resource = scope.task_resource(&child).await?;
		scope.require(&resource, "task.delegate").await?;
		let node = scope.node_resource(&onward.node);
		scope.require(&node, "federation.execute").await
	}
	.await;
	match checked {
		Ok(()) => scope.finish(Ok(())).await?,
		Err(error) => {
			scope.finish(Err(error)).await?;
			return Err(Error::External("hand-on scope accepted a refusal".into()));
		}
	}
	let delegation = repository.delegate(chain, &onward).await?;
	let result = serde_json::to_value(delegation)?;
	// The child is handed on whether or not the answer is saved, so a failed
	// save does not turn it into a refusal. Only a repeat of this command after
	// the target claimed the child would then be refused, as before replays.
	if let Err(error) = repository.save_replay(grant, &onward.replay, &result).await {
		tracing::warn!(%error, grant = %grant, child = %onward.child, "onward delegation replay not saved");
	}
	Ok(result)
}
async fn track(
	scope: &mut dyn RemoteCommandEffects,
	result: &Applied,
	grant: Uuid,
	thread: Uuid,
	kind: &str,
) -> Result<()> {
	let id = result
		.output_id
		.ok_or_else(|| Error::External("remote output identifier missing".into()))?;
	scope.record_output(grant, thread, kind, id).await
}
#[cfg(test)]
mod tests;

/// The Tasks a completion or an abandonment names; absent means none.
fn abandon_list(data: &Value) -> Result<Vec<Uuid>> {
	Ok(match data.get("abandon") {
		None | Some(Value::Null) => Vec::new(),
		Some(value) => serde_json::from_value(value.clone())?,
	})
}

/// A Home-forwarded call carries exactly the fields its tool defines, so nothing in the
/// request can steer where the Home reads or writes.
fn only_fields(data: &Value, allowed: &[&str]) -> Result<()> {
	let Some(object) = data.as_object() else {
		return Err(Error::Invalid("request data must be an object".into()));
	};
	if let Some(unknown) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
		return Err(Error::Invalid(format!("unknown request field: {unknown}")));
	}
	Ok(())
}

/// The prompt the `connection_request` tool composes, checked again on the Home.
fn require_connection_prompt(prompt: &str) -> Result<()> {
	let value: Value = serde_json::from_str(prompt)
		.map_err(|_| Error::Invalid("invalid connection request".into()))?;
	if value["tool"] != "connection_request" {
		return Err(Error::Invalid("invalid connection request".into()));
	}
	let resource = value["resource"]
		.as_str()
		.ok_or_else(|| Error::Invalid("invalid connection request".into()))?;
	if aidash_domain::oauth_connections::canonical_resource(resource)? != resource {
		return Err(Error::Invalid("invalid connection request".into()));
	}
	let scopes: Vec<String> =
		serde_json::from_value(value.get("scopes").cloned().unwrap_or_else(|| json!([])))?;
	aidash_domain::oauth_connections::admissible_scopes(&scopes, None, None, None)?;
	Ok(())
}

/// Abandoning needs `task.abandon` on each Task for the caller's chain, and a
/// child of the Run's own Task in its Thread. Who created it is not asked
/// here: an earlier Run of the same parent, under another grant, may have, and
/// the Home re-checks that and everything else inside the row lock.
async fn require_abandonment(
	scope: &mut dyn RemoteCommandEffects,
	parent: &aidash_domain::Task,
	ids: &[Uuid],
) -> Result<()> {
	for id in ids {
		let child = scope.task(*id).await?;
		if child.thread_id != parent.thread_id || child.parent_id != Some(parent.id) {
			return Err(Error::Forbidden);
		}
		let resource = scope.task_resource(&child).await?;
		scope.require(&resource, "task.abandon").await?;
	}
	Ok(())
}
