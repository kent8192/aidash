use crate::{
	Error, Result,
	authorization::execution::{self, Guard},
	context::{self, Context},
	domain::*,
	federation::{Federation, Home},
	provider::{ModelResponse, provider},
	registry::{AgentConfig, ModelConfig},
	tool::{PluginTool, Tool, ToolConfig, ToolContext, builtins},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use uuid::Uuid;

const POST_TOOL_CONTEXT_RESERVE: usize = 4096;
const TOOL_EVENT_RESERVE: usize = 512;
const RUN_MESSAGE_SUMMARY_OUTPUT_LIMIT: u32 = 2048;

enum WorkspaceReadFit {
	Skip,
	NoEnvelopeRoom,
	Fitted {
		call: crate::provider::ToolCall,
		result: Value,
	},
}

#[derive(Clone, Copy)]
struct WorkspaceReadFitBudget {
	requested: usize,
	offset: usize,
	request_tokens: usize,
	request_window: usize,
	remaining_calls: usize,
}

#[derive(Clone, Copy)]
struct WorkspaceReadRange {
	offset: usize,
	requested: usize,
}

struct RunMessagePage {
	entries: Vec<(usize, bool)>,
	has_more: bool,
}

fn run_message_page(
	inputs: &[crate::store::RunInput],
	after_seq: i64,
	limit: usize,
	references_only: bool,
) -> Result<RunMessagePage> {
	let mut entries = Vec::new();
	let mut encoded_size = 2_usize; // JSON array brackets.
	let mut content_size = 0_usize;
	let mut has_more = false;
	for (index, input) in inputs
		.iter()
		.enumerate()
		.filter(|(_, input)| input.seq > after_seq)
	{
		let id = input
			.message_id
			.ok_or_else(|| Error::External("run message home delivery is pending".into()))?;
		let full = json!({"seq":input.seq,"sender":input.sender,"content":input.content});
		let full_size = full.to_string().len();
		let reference = json!({"seq":input.seq,"sender":input.sender,"record":{"kind":"message","id":id},"requires_workspace_read":true});
		let mut reference_only = references_only || input.reference_only;
		let mut entry = if reference_only {
			reference.clone()
		} else {
			full
		};
		let comma_size = usize::from(!entries.is_empty());
		let mut next_size = encoded_size
			.saturating_add(comma_size)
			.saturating_add(entry.to_string().len());
		let next_content_size = content_size.saturating_add(full_size);
		if next_size > limit && !reference_only {
			reference_only = true;
			entry = reference;
			next_size = encoded_size
				.saturating_add(comma_size)
				.saturating_add(entry.to_string().len());
		}
		if next_size > limit || (!entries.is_empty() && next_content_size > limit) {
			if entries.is_empty() {
				if next_size > limit {
					return Err(Error::Invalid(
						"run message reference exceeds the model context limit".into(),
					));
				}
			} else {
				has_more = true;
				break;
			}
		}
		encoded_size = next_size;
		content_size = next_content_size;
		entries.push((index, reference_only));
	}
	Ok(RunMessagePage { entries, has_more })
}

fn request_context_window(window: usize, minimum_request: usize) -> usize {
	let available = window.saturating_sub(minimum_request);
	let reserve =
		POST_TOOL_CONTEXT_RESERVE.min(available.saturating_sub(context::MIN_CONTEXT_RESERVE) / 4);
	window.saturating_sub(reserve.saturating_mul(2))
}

fn message_read_range(event: &Value) -> Option<(String, usize, usize, usize)> {
	let call = &event["call"];
	let output = &event["result"];
	if event["kind"] != "tool"
		|| call["name"] != "workspace_read"
		|| call["arguments"]["kind"] != "message"
		|| output["kind"] != "message"
		|| output["encoding"] != "json"
		|| call["arguments"]["id"] != output["id"]
	{
		return None;
	}
	let id = output["id"].as_str()?.to_owned();
	let start = output["offset"].as_u64()? as usize;
	let total = output["total_chars"].as_u64()? as usize;
	let content = output["content"].as_str()?;
	let end = start.checked_add(content.chars().count())?;
	let next = output["next_offset"].as_u64().map(|value| value as usize);
	(end <= total && end > start && next.unwrap_or(total) == end).then_some((id, start, end, total))
}

fn record_message_read_in(
	coverage_by_id: &mut BTreeMap<String, context::MessageReadCoverage>,
	event: &Value,
) {
	let Some((id, start, end, total)) = message_read_range(event) else {
		return;
	};
	let coverage = coverage_by_id.entry(id).or_default();
	if coverage.total_chars != total {
		coverage.total_chars = total;
		coverage.ranges.clear();
	}
	coverage.ranges.push([start, end]);
	coverage.ranges.sort_unstable_by_key(|range| range[0]);
	let mut merged: Vec<[usize; 2]> = Vec::with_capacity(coverage.ranges.len());
	for range in coverage.ranges.drain(..) {
		if let Some(last) = merged.last_mut()
			&& range[0] <= last[1]
		{
			last[1] = last[1].max(range[1]);
		} else {
			merged.push(range);
		}
	}
	coverage.ranges = merged;
}

fn record_message_read(context: &mut Context, event: &Value) {
	record_message_read_in(&mut context.message_read_coverage, event);
}

fn capture_message_read_coverage(context: &mut Context) {
	for event in context.history.clone() {
		record_message_read(context, &event);
	}
}

fn capture_message_inference_coverage(context: &mut Context) {
	for event in context.history.clone() {
		record_message_read_in(&mut context.message_inference_coverage, &event);
	}
}

fn coverage_complete(
	coverage_by_id: &BTreeMap<String, context::MessageReadCoverage>,
	id: Uuid,
) -> bool {
	coverage_by_id.get(&id.to_string()).is_some_and(|coverage| {
		coverage.total_chars > 0
			&& coverage.ranges.len() == 1
			&& coverage.ranges[0] == [0, coverage.total_chars]
	})
}

fn referenced_message_read(context: &Context, id: Uuid) -> bool {
	coverage_complete(&context.message_read_coverage, id)
}

fn referenced_message_inferred(context: &Context, id: Uuid) -> bool {
	coverage_complete(&context.message_inference_coverage, id)
}

fn is_required_message_read(
	call: &crate::provider::ToolCall,
	required_reads: &[Uuid],
	context: &Context,
) -> bool {
	if call.name != "workspace_read" || call.arguments["kind"] != "message" {
		return false;
	}
	let Some(id) = call.arguments["id"]
		.as_str()
		.and_then(|id| id.parse::<Uuid>().ok())
	else {
		return false;
	};
	if !required_reads.contains(&id) || referenced_message_read(context, id) {
		return false;
	}
	let next_offset = context
		.message_read_coverage
		.get(&id.to_string())
		.and_then(|coverage| {
			coverage
				.ranges
				.iter()
				.find(|range| range[0] == 0)
				.map(|range| range[1])
		})
		.unwrap_or(0);
	call.arguments["offset"].as_u64().unwrap_or(0) as usize == next_offset
}

#[derive(Clone)]
pub struct Harness {
	pub federation: Federation,
}
impl Harness {
	pub async fn worker_once(&self) -> Result<bool> {
		let store = &self.federation.store;
		// Terminal runs are no longer leased, but their accepted remote inputs
		// remain in the durable outbox until home delivery is acknowledged.
		if let Some(run) = store.pending_terminal_run_message().await? {
			match self.federation.deliver_run_messages(&run).await {
				Ok(()) => return Ok(true),
				Err(error) => {
					tracing::warn!(run_id=%run.id, %error, "terminal run message delivery deferred");
					store.defer_run_message_delivery(run.id).await?;
				}
			}
		}
		let mut visibility = crate::transactions::gate::ReadLease::begin(store).await?;
		let token = Uuid::new_v4();
		let Some(mut run) = store
			.lease_run(token, self.federation.config.lease_seconds)
			.await?
		else {
			return Ok(false);
		};
		let result = {
			let work = self.advance(&mut run, token, &mut visibility);
			tokio::pin!(work);
			let mut heartbeat = tokio::time::interval(Duration::from_secs(
				(self.federation.config.lease_seconds / 3).max(1) as u64,
			));
			heartbeat.tick().await;
			loop {
				tokio::select! {
					result=&mut work=>break result,
					_=heartbeat.tick()=>{
						if !renew_worker_lease(store, run_id(store, token).await?, token, self.federation.config.lease_seconds).await? {
							return Ok(true);
						}
					}
				}
			}
		};
		visibility.resume(store).await?;
		if let Err(e) = result {
			let id = match run_id(store, token).await {
				Ok(id) => id,
				Err(_) => return Ok(true),
			};
			let mut current = store.run(id).await?;
			let attempts = current.pending["retry_count"].as_u64().unwrap_or(0) + 1;
			if matches!(
				e,
				Error::Forbidden | Error::Unauthorized | Error::IdentityStatusUnavailable
			) {
				let reason = if matches!(e, Error::IdentityStatusUnavailable) {
					"identity status unavailable"
				} else {
					"execution authority denied"
				};
				store
					.pause_for_authorization(&current, token, reason)
					.await?;
			} else if matches!(e, Error::TransactionPending | Error::StaleInference) {
				current.pending["retry_at"] =
					json!(chrono::Utc::now() + chrono::Duration::seconds(1));
				store.save_run(&current, token, "run.retrying").await?;
			} else if current.pending.get("terminal_transition").is_some() {
				// Delivery is durable and unbounded; never retry the failed tool
				// just because its home node has not acknowledged terminal state.
				current.pending["last_delivery_error"] = json!(e.to_string());
				current.pending["wake_at"] =
					json!(chrono::Utc::now() + chrono::Duration::seconds(5));
				store
					.save_run(&current, token, "run.failure_pending")
					.await?;
			} else if matches!(e, Error::External(_))
				&& attempts <= 5
				&& current.control != "CANCELLED"
			{
				current.pending["retry_count"] = json!(attempts);
				current.pending["retry_at"] = json!(
					chrono::Utc::now() + chrono::Duration::seconds(2_i64.pow(attempts as u32))
				);
				current.error = Some(e.to_string());
				store.save_run(&current, token, "run.retrying").await?;
			} else {
				let target = if current.control == "CANCELLED" {
					"CANCELLED"
				} else {
					"FAILED"
				};
				current.phase = "WAITING".into();
				current.pending =
					json!({"terminal_transition":target,"wake_at":chrono::Utc::now()});
				current.error = Some(e.to_string());
				store
					.save_run(&current, token, "run.failure_pending")
					.await?;
			}
		}
		Ok(true)
	}
	pub async fn run_worker(&self) -> Result<()> {
		let (_sender, receiver) = tokio::sync::watch::channel(false);
		self.run_worker_until(receiver).await
	}
	/// Finish the current durable step, then stop claiming work on shutdown.
	pub async fn run_worker_until(
		&self,
		stopping: tokio::sync::watch::Receiver<bool>,
	) -> Result<()> {
		while !*stopping.borrow() {
			match self.worker_once().await {
				Ok(true) => {}
				Ok(false) => {
					tokio::select! {_=tokio::time::sleep(Duration::from_millis(500))=>{},_=self.federation.notify.notified()=>{}}
				}
				Err(Error::TransactionPending) => {
					tokio::time::sleep(Duration::from_millis(250)).await;
				}
				Err(e) => {
					tracing::error!(error=%e,"worker step failed");
					tokio::time::sleep(Duration::from_secs(1)).await;
				}
			}
		}
		Ok(())
	}
	async fn tool_error(
		&self,
		run: &mut Run,
		token: Uuid,
		call: &crate::provider::ToolCall,
		cursor: usize,
		message: String,
	) -> Result<()> {
		let mut context: Context = serde_json::from_value(run.context.clone())?;
		let event = json!({"kind":"tool","call":call,"result":{"error":message}});
		let growth = context::tool_event_growth(&context, &event);
		context.history.push(event);
		run.pending["request_tokens"] = json!(
			run.pending["request_tokens"]
				.as_u64()
				.unwrap_or(0)
				.saturating_add(growth as u64)
		);
		run.context = json!(context);
		run.pending["cursor"] = json!(cursor + 1);
		self.federation
			.store
			.save_run(run, token, "run.tool_recorded")
			.await?;
		Ok(())
	}

	async fn tools(&self, config: &AgentConfig) -> Result<BTreeMap<String, Arc<dyn Tool>>> {
		let mut tools = builtins();
		crate::capabilities::tools::add(&mut tools, &config.core_capabilities);
		for (index, reference) in config.tools.iter().enumerate() {
			let entry = self
				.federation
				.registry
				.get(&reference.id, &reference.version)
				.await?;
			let cfg: ToolConfig = serde_json::from_value(entry.config.clone())?;
			let alias = format!("plugin_{index}");
			tools.insert(
				alias.clone(),
				Arc::new(PluginTool {
					entry,
					alias,
					config: cfg,
					client: self.federation.client.clone(),
				}),
			);
		}
		Ok(tools)
	}
	async fn publish_model_text(
		&self,
		home: &Home,
		guard: Option<&Guard>,
		run: &Run,
		worker: Uuid,
		text: &str,
	) -> Result<()> {
		if text.is_empty() {
			return Ok(());
		}
		if let Some(guard) = guard {
			guard
				.action("message.create", "workspace", run.workspace_id)
				.await?;
		}
		home.response_message(
			worker,
			run.pending["included_input_seq"]
				.as_i64()
				.unwrap_or(run.observed_input_seq),
			&format!(
				"{}:{}:output",
				run.id,
				run.pending["response_epoch"]
					.as_i64()
					.unwrap_or(run.revision)
			),
			text,
		)
		.await
	}
	async fn advance(
		&self,
		run: &mut Run,
		token: Uuid,
		visibility: &mut crate::transactions::gate::ReadLease,
	) -> Result<()> {
		if execution::cancel_if_scoped(&self.federation.store, run, token).await? {
			return Ok(());
		}
		let guard = Guard::begin(&self.federation, run).await?;
		let result = self
			.advance_step(run, token, guard.as_ref(), visibility)
			.await;
		if let Some(guard) = guard {
			guard.finish(result).await
		} else {
			result
		}
	}
	async fn advance_step(
		&self,
		run: &mut Run,
		token: Uuid,
		guard: Option<&Guard>,
		visibility: &mut crate::transactions::gate::ReadLease,
	) -> Result<()> {
		let store = &self.federation.store;
		let home = Home::new(self.federation.clone(), run.clone())
			.with_authority(guard.and_then(Guard::local_authority));
		// Accepted remote inputs remain deliverable even when the home task has
		// already reached a terminal state. Drain them before terminal recovery.
		self.federation.deliver_run_messages(run).await?;
		let task = home.task().await?;
		if matches!(
			task.status.as_str(),
			"COMPLETED" | "FAILED" | "CANCELLED" | "ABANDONED"
		) {
			run.phase = if task.status == "ABANDONED" {
				"CANCELLED".into()
			} else {
				task.status
			};
			run.pending = json!({});
			let kind = if run.phase == "FAILED" {
				"run.failed"
			} else {
				"run.reconciled"
			};
			store.save_run(run, token, kind).await?;
			return Ok(());
		}
		if let Some(target) = run.pending["terminal_transition"]
			.as_str()
			.map(str::to_owned)
		{
			self.federation
				.transition_terminal_run_messages(run, &target)
				.await?;
			run.phase = target;
			run.pending = json!({});
			let kind = if run.phase == "FAILED" {
				"run.failed"
			} else {
				"run.cancelled"
			};
			store.save_run(run, token, kind).await?;
			return Ok(());
		}
		if let Some(object) = run.pending.as_object_mut() {
			object.remove("retry_at");
			if object.remove("lease_recovered").is_some() {
				let data = json!({"run_id":run.id,"task_id":run.task_id,"phase":run.phase,"cause":"expired worker lease"});
				store
					.emit(
						home.local().then_some(run.workspace_id),
						"run.recovered",
						data.clone(),
					)
					.await?;
				home.report(
					&format!("{}:{}:recovered", run.id, run.revision),
					"remote.run.recovered",
					data,
				)
				.await?;
			}
		}
		if run.control == "CANCELLED" {
			self.federation
				.transition_terminal_run_messages(run, "CANCELLED")
				.await?;
			run.phase = "CANCELLED".into();
			store.save_run(run, token, "run.cancelled").await?;
			return Ok(());
		}
		let entry = self
			.federation
			.registry
			.get(&run.agent_id, &run.agent_version)
			.await?;
		let agent: AgentConfig = serde_json::from_value(entry.config.clone())?;
		match run.phase.as_str() {
			"READY" => {
				let task = home.task().await?;
				if !task.dependencies.is_empty() {
					let mut waiting = false;
					for id in &task.dependencies {
						let dependency: Task = serde_json::from_value(
							home.read_record("task", &id.to_string()).await?,
						)?;
						if matches!(
							dependency.status.as_str(),
							"FAILED" | "CANCELLED" | "ABANDONED"
						) {
							return Err(Error::Invalid(format!(
								"dependency {} is {}",
								dependency.id, dependency.status
							)));
						}
						waiting |= dependency.status != "COMPLETED";
					}
					if waiting {
						run.phase = "WAITING".into();
						run.pending = json!({"wake_at":chrono::Utc::now()+chrono::Duration::seconds(2),"resume_phase":"READY"});
						store.save_run(run, token, "run.waiting").await?;
						return Ok(());
					}
				}
				home.claim(&task, &entry).await?;
				home.transition("RUNNING").await?;
				run.phase = "THINKING".into();
				store.save_run(run, token, "run.started").await?;
			}
			"THINKING" => {
				self.federation.reconcile_run_messages(run).await?;
				// An accepted remote correction is durable even if its first home
				// delivery failed. Deliver it before building any inference request.
				self.federation.deliver_run_messages(run).await?;
				let force_read_compaction =
					run.pending["force_workspace_read_compaction"].as_bool() == Some(true);
				if let Some(guard) = guard {
					guard.inference().await?;
				}
				if run.step >= agent.max_steps {
					return Err(Error::Invalid("agent max_steps exceeded".into()));
				}
				let model_entry = self
					.federation
					.registry
					.get(&agent.model.id, &agent.model.version)
					.await?;
				let model_cfg: ModelConfig = serde_json::from_value(model_entry.config)?;
				let window = model_cfg.context_window;
				let output_limit = model_cfg.output_token_limit();
				let model = provider(self.federation.client.clone(), model_cfg)?;
				let mut tools = self.tools(&agent).await?;
				if let Some(guard) = guard {
					guard.filter_core_tools(&mut tools).await?;
				}
				let task = home.task().await?;
				if task.status == "COMPLETED" {
					run.phase = "COMPLETED".into();
					store.save_run(run, token, "run.recovered").await?;
					return Ok(());
				}
				let mut instructions = crate::context::agent_instructions("");
				for skill in &agent.skills {
					let entry = self
						.federation
						.registry
						.get(&skill.id, &skill.version)
						.await?;
					instructions.push('\n');
					instructions.push_str(&format!("Skill {}@{}:\n", skill.id, skill.version));
					instructions.push_str(&crate::registry::skill_instructions(&entry)?);
				}
				if tools.contains_key("skill_list")
					&& let Some(authority) = &home.authority
				{
					instructions.push_str(&authority.skill_context(store, run).await?);
				}
				instructions.push_str("\nAdditional user instructions:\n");
				instructions.push_str(&agent.instructions);
				let documents =
					crate::knowledge::load(&self.federation.registry.db, &entry).await?;
				let mut context: Context = serde_json::from_value(run.context.clone())?;
				capture_message_read_coverage(&mut context);
				let observation = home
					.observation(0, context::observation::DEFAULT_LIMIT)
					.await?;
				let inputs = store.run_inputs(run.id).await?;
				let input_seq = inputs
					.last()
					.map_or(run.observed_input_seq, |input| input.seq);
				let summary_seq = context.run_message_summary_seq;
				let run_message_limit = self.federation.run_message_limit(run).await?;
				let initial_page =
					run_message_page(&inputs, summary_seq, run_message_limit, false)?;
				let has_unprocessed_inputs = inputs.iter().any(|input| input.seq > summary_seq);
				let run_message_catchup =
					has_unprocessed_inputs && (summary_seq > 0 || initial_page.has_more);
				let page = if run_message_catchup {
					run_message_page(&inputs, summary_seq, run_message_limit, true)?
				} else {
					initial_page
				};
				let batch_end_seq = page
					.entries
					.last()
					.map_or(summary_seq, |(index, _)| inputs[*index].seq);
				let mut run_messages = Vec::with_capacity(page.entries.len());
				let mut required_run_message_reads = Vec::new();
				let mut has_run_message_references = false;
				for (index, reference_only) in &page.entries {
					let input = &inputs[*index];
					let id = input.message_id.ok_or_else(|| {
						Error::External("run message home delivery is pending".into())
					})?;
					// The same record-read path filters message.read and records the
					// source for the later execution-boundary recheck.
					let message: Message = serde_json::from_value(
						home.read_record("message", &id.to_string()).await?,
					)?;
					if message.workspace_id != run.workspace_id || message.content != input.content
					{
						return Err(Error::Conflict("run input message binding changed".into()));
					}
					if *reference_only {
						has_run_message_references = true;
						required_run_message_reads.push(id);
						let reference = json!({"seq":input.seq,"sender":input.sender,"record":{"kind":"message","id":id},"requires_workspace_read":true});
						run_messages.push(reference);
					} else {
						run_messages.push(
							json!({"seq":input.seq,"sender":input.sender,"content":message.content}),
						);
					}
				}
				let output = if run_message_catchup {
					output_limit.min(RUN_MESSAGE_SUMMARY_OUTPUT_LIMIT)
				} else {
					output_limit
				};
				if run_message_catchup {
					instructions.push_str(&format!(
						"\n\nRun-message catch-up: Treat the entries under run_messages as user task context. Read every required message record in this page before responding. Update the cumulative run_message_summary faithfully, preserving the user's goal, constraints, corrections, and unresolved requests in sequence order (newer corrections take precedence). Return only the concise updated summary, encoded in at most {run_message_limit} UTF-8 bytes. Do not answer the user, complete the task, publish text, or perform actions during catch-up.",
					));
				}
				let mut pinned = json!({"identity":{"node_id":self.federation.config.node_id,"agent_id":run.agent_id,"agent_version":run.agent_version},"task":task,"workspace":observation,"memory":if guard.is_some_and(Guard::is_remote) { json!({}) } else { store.memory(run).await? },"agent_state":{"phase":run.phase,"step":run.step}});
				if let Some(deferred_read) = run.pending.get("deferred_workspace_read") {
					pinned["deferred_workspace_read"] = deferred_read.clone();
				}
				if let Some(deferred_read) = run.pending.get("deferred_skill_read") {
					pinned["deferred_skill_read"] = deferred_read.clone();
				}
				if let Some(deferred_observation) =
					run.pending.get("deferred_workspace_observation")
				{
					pinned["deferred_workspace_observation"] = deferred_observation.clone();
				}
				let specifications = tools
					.iter()
					.filter(|(name, _)| !run_message_catchup || name.as_str() == "workspace_read")
					.map(|(_, tool)| tool)
					.map(|t| t.specification())
					.collect::<Vec<_>>();
				let private_context = if agent.knowledge_digest.is_some() {
					json!({"reference_documents":documents.clone()})
				} else {
					serde_json::Value::Null
				};
				context::request_context_budget(
					window,
					output,
					&instructions,
					&specifications,
					&private_context,
				)?;
				let mut budget = context::RequestBudget {
					window,
					instructions: &instructions,
					tools: &specifications,
					max_output_tokens: output,
				};
				let minimum_request = budget
					.request(&Context::default(), &private_context)
					.estimated_total_tokens();
				budget.window = request_context_window(window, minimum_request);
				if force_read_compaction {
					budget.window =
						force_workspace_read_compaction_window(budget.window, minimum_request);
				}
				// Keep room for history and JSON message escaping. The final fitting
				// decision below measures the complete provider input, not this quota.
				let available = budget.remaining(&Context::default(), &private_context);
				let message_size = context::estimated_tokens(&json!(run_messages).to_string());
				let snapshot_fit = context::bound_snapshot(
					&mut pinned,
					available.saturating_sub(message_size) / 4,
				);
				if agent.knowledge_digest.is_some() {
					pinned["reference_documents"] = documents;
				}
				// The snapshot quota is a heuristic. Required IDs and other minimum
				// context may exceed it while the complete request still fits.
				// Keep the authorized run-directed input independent of the bounded
				// workspace preview. Admission has already capped its aggregate size.
				if !run_messages.is_empty() {
					pinned["run_messages"] = json!(run_messages);
					if has_run_message_references {
						pinned["run_message_read_instruction"] = if run_message_catchup {
							json!(
								"Read every run_messages entry with requires_workspace_read through workspace_read(kind=message, id=record.id) before returning the updated run_message_summary. Its full content remains in that workspace record."
							)
						} else {
							json!(
								"Read every run_messages entry with requires_workspace_read through workspace_read(kind=message, id=record.id) before completing the task. Its full content remains in that workspace record."
							)
						};
					}
				}
				if let Err(error) = snapshot_fit
					&& budget
						.request(&Context::default(), &pinned)
						.estimated_total_tokens()
						> budget.window
				{
					return Err(error);
				}
				let semantic_budget = budget.remaining(&Context::default(), &pinned) / 2;
				if let Some(guard) = guard {
					if let Some(semantic) = guard
						.semantic_context(
							store,
							&format!("{}\n{}", task.title, task.description),
							semantic_budget,
						)
						.await?
					{
						pinned["semantic_memory"] = json!(semantic);
					}
				} else if home.local() {
					let mut lease = crate::semantic::service::Lease::begin(
						store,
						&crate::authorization::identity::Actor::Operator,
					)
					.await?;
					let result = crate::semantic::service::context_in(
						store,
						&mut lease,
						run,
						&format!("{}\n{}", task.title, task.description),
						semantic_budget,
					)
					.await;
					if let Some(semantic) = lease.finish(result).await? {
						pinned["semantic_memory"] = json!(semantic);
					}
				}

				let compactor: Box<dyn context::jev::JevAsker> = if let Some(guard) = guard {
					Box::new(guard.compactor(&self.federation))
				} else {
					Box::new(context::jev::JevClient::from_env(
						self.federation.client.clone(),
					)?)
				};
				context::compact(&mut context, compactor.as_ref(), &budget, &pinned).await?;
				if let Some(guard) = guard {
					guard.inference().await?;
				}
				let request = budget.request(&context, &pinned);
				crate::generation::budget::Reservation::check_request(window, &request)?;
				let request_tokens = request.estimated_total_tokens();
				let reservation = if let Some(guard) = guard {
					guard
						.reserve_inference(store, token, window, output)
						.await?
				} else {
					None
				};
				// Race only inference, not replay-unsafe tools or durable transitions.
				// Release node-wide visibility and authorization row locks while the
				// provider waits; both boundaries are reacquired before accepting output.
				if let Some(guard) = guard {
					guard.suspend().await?;
				}
				visibility.suspend().await?;
				let result = tokio::select! {
					biased;
					cancelled = wait_for_inference_cancellation(store, run.id) => match cancelled {
						Ok(()) => Err(Error::Conflict("run cancelled during inference".into())),
						Err(error) => Err(error),
					},
					result = model.infer(request) => result,
				};
				let resumed = visibility.resume(store).await;
				if let (Some(reservation), Ok(response)) = (reservation, result.as_ref()) {
					// Provider usage is billable even when authorization changed
					// or a transaction committed while its result was in flight.
					reservation.settle(response).await?;
				}
				resumed?;
				let result = result?;
				if let Some(guard) = guard {
					guard.resume(&self.federation).await?;
				}
				// Count only tool content that survived compaction and was present
				// in a successful provider request, not every completed read.
				capture_message_inference_coverage(&mut context);
				context.usage = json!({"input_tokens":result.input_tokens,"output_tokens":result.output_tokens,"context_window":window,"compactions":context.compactions});
				run.context = json!(context);
				let references_read_at_inference = required_run_message_reads
					.iter()
					.all(|id| referenced_message_inferred(&context, *id));
				if references_read_at_inference && !run_message_catchup {
					run.observed_input_seq = input_seq;
				}
				run.pending = json!({
					"response":result,
					"response_epoch":response_epoch(run.revision, run.step),
					"cursor":0,
					"included_input_seq":input_seq,
					"request_window":budget.window,
					"request_tokens":request_tokens,
					"required_run_message_reads":required_run_message_reads,
					"references_read_at_inference":references_read_at_inference,
					"run_message_catchup":run_message_catchup,
					"run_message_summary_end_seq":batch_end_seq,
					"run_message_summary_limit":run_message_limit
				});
				run.phase = "TOOL_CALL".into();
				run.error = None;
				store.save_run(run, token, "model.completed").await?;
			}
			"TOOL_CALL" => {
				// An old home replica can still accept a correction directly from
				// an old executor. Wait for its upgraded database gate before any
				// model output or final completion crosses this boundary.
				match self.federation.require_terminal_safe_delivery(run).await {
					Ok(()) => {}
					Err(Error::Conflict(_)) => return Err(Error::TransactionPending),
					Err(error) => return Err(error),
				}
				// A preceding binary could have left a remote correction only on
				// the home node while this run was already awaiting finalization.
				self.federation.reconcile_run_messages(run).await?;
				let included_input_seq = run.pending["included_input_seq"]
					.as_i64()
					.unwrap_or(run.observed_input_seq);
				if store
					.run_inputs(run.id)
					.await?
					.last()
					.is_some_and(|input| input.seq > included_input_seq)
				{
					// This response predates an accepted correction. Discard its
					// text and calls before any effect crosses the tool boundary. Keep
					// the inference allowance and let the next stored response get a
					// fresh response_epoch for idempotency keys.
					run.phase = "THINKING".into();
					run.pending = json!({});
					store.save_run(run, token, "run.message_received").await?;
					return Ok(());
				}
				if run.pending["response_epoch"].as_i64().is_none() {
					// Older responses use step-based idempotency keys. The first
					// invocation persists this value atomically with its tool input;
					// final output can safely replay the same deterministic key.
					run.pending["response_epoch"] = json!(run.step);
				}
				let mut result: ModelResponse =
					serde_json::from_value(run.pending["response"].clone())?;
				let run_message_catchup = run.pending["run_message_catchup"] == true;
				if run_message_catchup {
					// A provider response cannot execute task tools during catch-up,
					// even if it returns calls that were not in the advertised tool set.
					result
						.tool_calls
						.retain(|call| call.name == "workspace_read");
				}
				let required_reads: Vec<Uuid> =
					serde_json::from_value(run.pending["required_run_message_reads"].clone())
						.unwrap_or_default();
				let mut context: Context = serde_json::from_value(run.context.clone())?;
				capture_message_read_coverage(&mut context);
				run.context = json!(context);
				let references_read = required_reads
					.iter()
					.all(|id| referenced_message_read(&context, *id));
				let references_inferred = required_reads
					.iter()
					.all(|id| referenced_message_inferred(&context, *id));
				let informed_response = required_reads.is_empty()
					|| (references_read
						&& references_inferred
						&& run.pending["references_read_at_inference"] == true);
				let cursor = run.pending["cursor"].as_u64().unwrap_or(0) as usize;
				if !informed_response
					&& let Some(call) = result.tool_calls.get(cursor)
					&& !is_required_message_read(call, &required_reads, &context)
				{
					context.history.push(
						json!({"kind":"run_message_read_required","message_ids":required_reads}),
					);
					run.context = json!(context);
					run.phase = "THINKING".into();
					if !run_message_catchup {
						run.step += 1;
					}
					run.pending = json!({});
					store
						.save_run(run, token, "run.message_read_required")
						.await?;
					return Ok(());
				}
				if cursor >= result.tool_calls.len() && !informed_response {
					context.history.push(
						json!({"kind":"run_message_read_required","message_ids":required_reads}),
					);
					run.context = json!(context);
					run.phase = "THINKING".into();
					if !run_message_catchup {
						run.step += 1;
					}
					run.pending = json!({});
					store
						.save_run(run, token, "run.message_read_required")
						.await?;
					return Ok(());
				}
				if cursor >= result.tool_calls.len() && run_message_catchup {
					let summary = result.text.trim().to_owned();
					if summary.is_empty() {
						context.history.push(json!({
							"kind":"run_message_summary_required",
							"through_seq":run.pending["run_message_summary_end_seq"]
						}));
						run.context = json!(context);
						run.phase = "THINKING".into();
						run.pending = json!({});
						store
							.save_run(run, token, "run.message_summary_required")
							.await?;
						return Ok(());
					}
					let summary_limit = run.pending["run_message_summary_limit"]
						.as_u64()
						.unwrap_or(0) as usize;
					if summary.len() > summary_limit {
						context.history.push(json!({
							"kind":"run_message_summary_required",
							"through_seq":run.pending["run_message_summary_end_seq"],
							"max_bytes":summary_limit,
							"reason":"summary exceeded the complete-summary limit"
						}));
						run.context = json!(context);
						run.phase = "THINKING".into();
						run.pending = json!({});
						store
							.save_run(run, token, "run.message_summary_required")
							.await?;
						return Ok(());
					}
					context.run_message_summary = summary;
					context.run_message_summary_seq = run.pending["run_message_summary_end_seq"]
						.as_i64()
						.ok_or_else(|| {
							Error::Conflict(
								"run message summary page is missing its sequence".into(),
							)
						})?;
					let summarized_ids = required_reads
						.iter()
						.map(ToString::to_string)
						.collect::<std::collections::BTreeSet<_>>();
					context.history.retain(|event| {
						message_read_range(event)
							.is_none_or(|(id, _, _, _)| !summarized_ids.contains(&id))
					});
					for id in &required_reads {
						let id = id.to_string();
						context.message_read_coverage.remove(&id);
						context.message_inference_coverage.remove(&id);
					}
					run.context = json!(context);
					run.phase = "THINKING".into();
					run.pending = json!({});
					store.save_run(run, token, "run.message_summarized").await?;
					return Ok(());
				}
				if !run_message_catchup && !result.tool_calls.is_empty() && informed_response {
					self.publish_model_text(&home, guard, run, token, &result.text)
						.await?;
				}
				if cursor >= result.tool_calls.len() {
					if result.tool_calls.is_empty() {
						let children = home.child_summary(run.task_id).await?;
						if children.has_pending {
							self.publish_model_text(&home, guard, run, token, &result.text)
								.await?;
							let failed = children.has_failed;
							if failed {
								if let Some(guard) = guard {
									guard.action("human.request", "run", run.id).await?;
								}
								let response_epoch = run.pending["response_epoch"]
									.as_i64()
									.unwrap_or(run.revision);
								let h=store.human_request(run,"INFORMATION_REQUEST","A subtask needs intervention. You can explicitly abandon failed, blocked or cancelled subtasks in their task details, providing a reason. Then answer this request to continue with the remaining results, or cancel this parent.",&format!("{}:{}:subtasks",run.id,response_epoch)).await?;
								run.pending =
									json!({"human_request_id":h.id,"resume_phase":"THINKING"});
							} else {
								run.pending = json!({"wake_at":chrono::Utc::now()+chrono::Duration::seconds(2),"resume_phase":"THINKING"});
							}
							run.step += 1;
							run.phase = "WAITING".into();
							store.save_run(run, token, "run.waiting").await?;
							return Ok(());
						}
						let artifact = ArtifactInput {
							kind: "text".into(),
							name: result_artifact_name(&home.task().await?.title),
							content: json!(result.text),
						};
						if let Some(guard) = guard {
							guard
								.action("artifact.create", "artifact", run.task_id)
								.await?;
							guard.action("task.complete", "task", run.task_id).await?;
						}
						if !store.begin_final_completion(run, token).await? {
							run.phase = "THINKING".into();
							run.pending = json!({});
							store.save_run(run, token, "run.message_received").await?;
							return Ok(());
						}
						self.publish_model_text(&home, guard, run, token, &result.text)
							.await?;
						if let Err(error) = home
							.complete(&format!("{}:complete", run.id), &artifact)
							.await
						{
							if matches!(error, Error::Conflict(_))
								&& home.child_summary(run.task_id).await?.has_pending
							{
								run.step += 1;
								run.phase = "WAITING".into();
								run.pending = json!({"wake_at":chrono::Utc::now()+chrono::Duration::seconds(2),"resume_phase":"THINKING"});
								store.save_run(run, token, "run.waiting").await?;
								return Ok(());
							}
							return Err(error);
						}
						run.phase = "COMPLETED".into();
						run.pending = json!({});
						store.save_run(run, token, "run.completed").await?;
					} else {
						run.phase = "THINKING".into();
						run.step += 1;
						let force_read_compaction = run.pending["force_workspace_read_compaction"]
							.as_bool()
							.unwrap_or(false);
						let deferred_read = run.pending.get("deferred_workspace_read").cloned();
						let deferred_skill_read = run.pending.get("deferred_skill_read").cloned();
						run.pending = if force_read_compaction {
							json!({
									"force_workspace_read_compaction":true,
									"deferred_workspace_read":deferred_read,
									"deferred_skill_read":deferred_skill_read
							})
						} else {
							json!({})
						};
						store.save_run(run, token, "run.thinking").await?;
					}
					return Ok(());
				}
				let mut context: Context = serde_json::from_value(run.context.clone())?;
				capture_message_read_coverage(&mut context);
				let mut call = result.tool_calls[cursor].clone();
				let mut prepared_result = None;
				if call.name == "workspace_read" {
					let (read_range, saved_read) =
						match workspace_read_plan_result(&call, run.step, cursor, &run.pending) {
							Ok(plan) => plan,
							Err(Error::Invalid(message)) => {
								return self.tool_error(run, token, &call, cursor, message).await;
							}
							Err(error) => return Err(error),
						};
					if let Some(output) = saved_read {
						prepared_result = Some(output.clone());
					} else {
						let request_tokens =
							run.pending["request_tokens"].as_u64().unwrap_or(0) as usize;
						let request_window =
							run.pending["request_window"].as_u64().unwrap_or(0) as usize;
						match cap_workspace_read(
							&home,
							&context,
							&call,
							read_range,
							request_tokens,
							request_window,
							result.tool_calls.len().saturating_sub(cursor + 1),
						)
						.await
						{
							Ok(WorkspaceReadFit::Skip) => {}
							Ok(WorkspaceReadFit::NoEnvelopeRoom) => {
								// Start a new inference turn without appending an error event:
								// even that envelope could exceed the smaller forced-compaction
								// quota on the next request.
								run.phase = "THINKING".into();
								if !run_message_catchup {
									run.step += 1;
								}
								run.pending = json!({
									"force_workspace_read_compaction":true,
									"deferred_workspace_read":deferred_workspace_read(&call)
								});
								store.save_run(run, token, "run.read_deferred").await?;
								return Ok(());
							}
							Ok(WorkspaceReadFit::Fitted {
								call: bounded,
								result: output,
							}) => {
								call = bounded;
								result.tool_calls[cursor] = call.clone();
								run.pending["response"] = json!(result);
								run.pending["workspace_read_plan"] = json!({
									"step":run.step,
									"cursor":cursor,
									"result":output
								});
								prepared_result = Some(output);
							}
							Err(Error::Invalid(message)) => {
								return self.tool_error(run, token, &call, cursor, message).await;
							}
							Err(error) => return Err(error),
						}
					}
				}
				if call.name == "skill_read" && call.arguments.get("skill").is_some() {
					let range = match skill_read_range(&call) {
						Ok(range) => range,
						Err(Error::Invalid(message)) => {
							return self.tool_error(run, token, &call, cursor, message).await;
						}
						Err(error) => return Err(error),
					};
					let saved_plan = &run.pending["skill_read_plan"];
					let saved_output = (saved_plan["step"].as_i64() == Some(run.step as i64)
						&& saved_plan["cursor"].as_u64() == Some(cursor as u64)
						&& saved_plan["call"] == json!(call))
					.then(|| saved_plan.get("result").cloned())
					.flatten();
					if let Some(output) = saved_output {
						prepared_result = Some(output);
					} else {
						let ctx = ToolContext {
							home: home.clone(),
							store: store.clone(),
							run: run.clone(),
						};
						let output = match builtins()
							.get("skill_read")
							.expect("skill_read builtin")
							.invoke(&ctx, call.arguments.clone(), "")
							.await
						{
							Ok(output) => output,
							Err(Error::Invalid(message)) => {
								return self.tool_error(run, token, &call, cursor, message).await;
							}
							Err(error) => return Err(error),
						};
						let request_tokens =
							run.pending["request_tokens"].as_u64().unwrap_or(0) as usize;
						let request_window =
							run.pending["request_window"].as_u64().unwrap_or(0) as usize;
						let budget = WorkspaceReadFitBudget {
							requested: range.requested,
							offset: range.offset,
							request_tokens,
							request_window,
							remaining_calls: result.tool_calls.len().saturating_sub(cursor + 1),
						};
						let Some(chars) = fit_skill_read_chars(&context, &call, &output, budget)
						else {
							run.phase = "THINKING".into();
							run.step += 1;
							run.pending = json!({
								"force_workspace_read_compaction":true,
								"deferred_skill_read":deferred_skill_read(&call)
							});
							store
								.save_run(run, token, "run.skill_read_deferred")
								.await?;
							return Ok(());
						};
						call.arguments["max_chars"] = json!(chars);
						result.tool_calls[cursor] = call.clone();
						run.pending["response"] = json!(result);
						let bounded = skill_read_result(&output, chars);
						run.pending["skill_read_plan"] = json!({
							"step":run.step,"cursor":cursor,"call":call,"result":bounded
						});
						prepared_result = Some(bounded);
					}
				}
				if call.name == "workspace_observe" {
					let saved_plan = &run.pending["workspace_observation_plan"];
					let saved_output = (saved_plan["step"].as_i64() == Some(run.step as i64)
						&& saved_plan["cursor"].as_u64() == Some(cursor as u64)
						&& saved_plan["call"] == json!(call))
					.then(|| saved_plan.get("result").cloned())
					.flatten();
					if let Some(output) = saved_output {
						prepared_result = Some(output);
					} else {
						let offset = call.arguments["offset"].as_u64().unwrap_or(0) as usize;
						let requested = call.arguments["limit"]
							.as_u64()
							.unwrap_or(context::observation::DEFAULT_LIMIT as u64)
							as usize;
						let request_tokens =
							run.pending["request_tokens"].as_u64().unwrap_or(0) as usize;
						let request_window =
							run.pending["request_window"].as_u64().unwrap_or(0) as usize;
						let remaining_calls = result.tool_calls.len().saturating_sub(cursor + 1);
						let fitted = home
							.observation_fitted(offset, requested, |limit, output| {
								Ok(workspace_observation_event_fits(
									&context,
									&call,
									limit,
									output,
									request_tokens,
									request_window,
									remaining_calls,
								))
							})
							.await?;
						let Some((limit, output)) = fitted else {
							// Retry after compaction without appending an observation event
							// that cannot fit in the next request quota.
							run.phase = "THINKING".into();
							run.step += 1;
							run.pending = json!({
								"force_workspace_read_compaction":true,
								"deferred_workspace_observation":deferred_workspace_observation(&call)
							});
							store
								.save_run(run, token, "run.observation_deferred")
								.await?;
							return Ok(());
						};
						call.arguments["limit"] = json!(limit);
						result.tool_calls[cursor] = call.clone();
						run.pending["response"] = json!(result);
						run.pending["workspace_observation_plan"] = json!({
							"step":run.step,
							"cursor":cursor,
							"call":call,
							"result":output
						});
						prepared_result = Some(output);
					}
				}
				let call = &call;
				let mut tools = self.tools(&agent).await?;
				if let Some(guard) = guard {
					guard.filter_core_tools(&mut tools).await?;
				}
				let Some(tool) = tools.get(&call.name) else {
					return self
						.tool_error(
							run,
							token,
							call,
							cursor,
							format!("unavailable tool {}", call.name),
						)
						.await;
				};
				if let Some(guard) = guard {
					match guard.tool(call).await {
						Ok(()) => {}
						Err(Error::Invalid(message)) => {
							return self.tool_error(run, token, call, cursor, message).await;
						}
						Err(error) => return Err(error),
					}
				}
				let response_epoch = run.pending["response_epoch"]
					.as_i64()
					.unwrap_or(run.revision);
				let key = format!("{}:{}:{}", run.id, response_epoch, cursor);
				let invocation = store
					.invocation_start(
						run,
						token,
						&key,
						&call.name,
						&call.arguments,
						tool.replay_safe(),
					)
					.await?;
				if invocation.status == "UNCERTAIN" {
					if let Some(guard) = guard {
						guard.action("human.request", "run", run.id).await?;
					}
					store.reconciliation_request(run, token, &key, &format!("Tool {} may have completed before the worker stopped. Reconcile the external effect, then answer with a JSON object containing result. It will not be executed again. Invocation: {key}",call.name)).await?;
					store.save_run(run, token, "run.waiting").await?;
					return Ok(());
				}
				let output = if invocation.status == "COMPLETED" {
					invocation.result.ok_or_else(|| {
						Error::Conflict("completed invocation has no result".into())
					})?
				} else if let Some(output) = prepared_result {
					output
				} else {
					let ctx = ToolContext {
						home: home.clone(),
						store: store.clone(),
						run: run.clone(),
					};
					match tool.invoke(&ctx, call.arguments.clone(), &key).await {
						Ok(output) => output,
						Err(Error::Invalid(message)) => json!({"error":message}),
						Err(e) => return Err(e),
					}
				};
				if invocation.status != "COMPLETED" {
					store.invocation_finish(run, token, &key, &output).await?;
				}
				let event = json!({"kind":"tool","call":call,"result":output});
				record_message_read(&mut context, &event);
				let growth = context::tool_event_growth(&context, &event);
				context.history.push(event);
				run.pending["request_tokens"] = json!(
					run.pending["request_tokens"]
						.as_u64()
						.unwrap_or(0)
						.saturating_add(growth as u64)
				);
				run.context = json!(context);
				if run.pending["workspace_read_plan"]["step"].as_u64() == Some(run.step as u64)
					&& run.pending["workspace_read_plan"]["cursor"].as_u64() == Some(cursor as u64)
					&& let Some(object) = run.pending.as_object_mut()
				{
					object.remove("workspace_read_plan");
				}
				if run.pending["skill_read_plan"]["step"].as_i64() == Some(run.step as i64)
					&& run.pending["skill_read_plan"]["cursor"].as_u64() == Some(cursor as u64)
					&& let Some(object) = run.pending.as_object_mut()
				{
					object.remove("skill_read_plan");
				}
				if run.pending["workspace_observation_plan"]["step"].as_i64()
					== Some(run.step as i64)
					&& run.pending["workspace_observation_plan"]["cursor"].as_u64()
						== Some(cursor as u64)
					&& let Some(object) = run.pending.as_object_mut()
				{
					object.remove("workspace_observation_plan");
				}
				run.pending["cursor"] = json!(cursor + 1);
				if output["status"] == "approval_required"
					&& let Some(id) = output.get("approval_id")
				{
					run.pending["core_approval_id"] = id.clone();
					run.pending["resume_phase"] = json!("THINKING");
					run.step += 1;
					run.phase = "WAITING".into();
				} else if call.name == "human_request"
					&& let Some(id) = output.get("human_request_id")
				{
					run.pending["human_request_id"] = id.clone();
					run.pending["resume_phase"] = json!("THINKING");
					run.step += 1;
					run.phase = "WAITING".into();
				} else if call.name == "workspace_wait"
					&& let Some(seconds) = output["wait_seconds"].as_i64()
				{
					run.pending["wake_at"] =
						json!(chrono::Utc::now() + chrono::Duration::seconds(seconds));
					run.pending["resume_phase"] = json!("TOOL_CALL");
					run.phase = "WAITING".into();
				}
				home.report(
					&format!("{key}:tool"),
					"remote.tool.completed",
					json!({"run_id":run.id,"call":call,"result":output}),
				)
				.await?;
				store.save_run(run, token, "run.tool_recorded").await?;
			}
			"WAITING" => {
				if let Some(id) = run.pending["human_request_id"].as_str() {
					let id = id
						.parse::<Uuid>()
						.map_err(|_| Error::Invalid("invalid pending human id".into()))?;
					if let Some(guard) = guard {
						guard.human_read(id).await?;
					}
					let h: HumanRequest = sqlx::query_as(
						&sea_orm::sea_query::Query::select()
							.expr(sea_orm::sea_query::SimpleExpr::from(
								sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
							))
							.from(sea_orm::sea_query::Alias::new("human_requests"))
							.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
							.to_string(sea_orm::sea_query::PostgresQueryBuilder),
					)
					.bind(id)
					.fetch_one(&store.pool)
					.await?;
					let response = h.response.ok_or_else(|| {
						Error::Conflict("human request has not been answered".into())
					})?;
					if let Some(key) = run.pending["uncertain_key"].as_str() {
						let result = response.get("result").ok_or_else(|| {
							Error::Invalid("reconciliation response must contain result".into())
						})?;
						store.invocation_finish(run, token, key, result).await?;
					} else {
						let mut context: Context = serde_json::from_value(run.context.clone())?;
						context.history.push(
							json!({"kind":"human","request":h.prompt,"request_kind":h.kind,"response":response}),
						);
						run.context = json!(context);
					}
				}
				run.phase = run.pending["resume_phase"]
					.as_str()
					.unwrap_or("THINKING")
					.into();
				for key in [
					"human_request_id",
					"core_approval_id",
					"uncertain_key",
					"wake_at",
					"resume_phase",
				] {
					run.pending.as_object_mut().unwrap().remove(key);
				}
				store.save_run(run, token, "run.resumed").await?;
			}
			_ => return Err(Error::Conflict("run is not executable".into())),
		}
		Ok(())
	}
}

async fn wait_for_inference_cancellation(store: &crate::store::Store, id: Uuid) -> Result<()> {
	use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};

	// Read committed control outside the step's authority lease. Notify is
	// process-local, and a long worker heartbeat must not delay cancellation.
	// Fetch only control rather than repeatedly copying the run's context.
	let query = Query::select()
		.column(Alias::new("control"))
		.from(Alias::new("runs"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
		.to_string(PostgresQueryBuilder);
	loop {
		let control = sqlx::query_scalar::<_, String>(&query)
			.bind(id)
			.fetch_one(&store.pool)
			.await;
		match control {
			Ok(control) if control == "CANCELLED" => return Ok(()),
			Ok(_) => {}
			Err(error) => {
				// A failed observation is not cancellation. Keep the in-flight
				// request alive; the existing heartbeat still fences the lease.
				tracing::warn!(
					run_id = %id,
					error = %error,
					"inference cancellation poll failed; retrying"
				);
			}
		}
		tokio::time::sleep(Duration::from_millis(250)).await;
	}
}

async fn renew_worker_lease(
	store: &crate::store::Store,
	run_id: Uuid,
	token: Uuid,
	lease_seconds: i32,
) -> Result<bool> {
	let mut delay = Duration::from_millis(250);
	loop {
		match store.renew_lease(run_id, token, lease_seconds).await {
			Ok(renewed) => return Ok(renewed),
			Err(error)
				if matches!(&error, Error::TransactionPending) || error.is_transient_database() =>
			{
				tracing::warn!(%error, run_id = %run_id, "retrying worker lease renewal after transient database error");
				tokio::time::sleep(delay).await;
				delay = delay.saturating_mul(2).min(Duration::from_secs(2));
			}
			Err(error) => return Err(error),
		}
	}
}

async fn cap_workspace_read(
	home: &Home,
	context: &Context,
	call: &crate::provider::ToolCall,
	range: WorkspaceReadRange,
	request_tokens: usize,
	request_window: usize,
	remaining_calls: usize,
) -> Result<WorkspaceReadFit> {
	let WorkspaceReadRange { offset, requested } = range;
	let (Some(kind), Some(id)) = (
		call.arguments["kind"].as_str(),
		call.arguments["id"].as_str(),
	) else {
		return Ok(WorkspaceReadFit::Skip);
	};
	let output = home.read_record_chunk(kind, id, offset, requested).await?;
	let Some(bounded) = fit_workspace_read_chars(
		context,
		call,
		&output,
		WorkspaceReadFitBudget {
			requested,
			offset,
			request_tokens,
			request_window,
			remaining_calls,
		},
	)?
	else {
		return Ok(WorkspaceReadFit::NoEnvelopeRoom);
	};
	let mut bounded_call = call.clone();
	bounded_call.arguments["max_chars"] = json!(bounded);
	Ok(WorkspaceReadFit::Fitted {
		call: bounded_call,
		result: workspace_read_result(&output, requested, offset, bounded),
	})
}

fn workspace_read_range(call: &crate::provider::ToolCall) -> Result<WorkspaceReadRange> {
	let offset = match call.arguments.get("offset") {
		None => 0,
		Some(value) => usize::try_from(value.as_u64().ok_or_else(|| {
			Error::Invalid("workspace read offset must be a nonnegative integer".into())
		})?)
		.map_err(|_| Error::Invalid("workspace read offset is too large".into()))?,
	};
	let requested = match call.arguments.get("max_chars") {
		None => 8000,
		Some(value) => {
			let requested = usize::try_from(value.as_u64().ok_or_else(|| {
				Error::Invalid("workspace read max_chars must be a nonnegative integer".into())
			})?)
			.map_err(|_| Error::Invalid("workspace read max_chars is too large".into()))?;
			if requested > 16_000 {
				return Err(Error::Invalid(
					"workspace read max_chars must not exceed 16000".into(),
				));
			}
			requested
		}
	};
	Ok(WorkspaceReadRange { offset, requested })
}

fn workspace_read_plan_result(
	call: &crate::provider::ToolCall,
	step: i32,
	cursor: usize,
	pending: &Value,
) -> Result<(WorkspaceReadRange, Option<Value>)> {
	let range = workspace_read_range(call)?;
	let saved_plan = &pending["workspace_read_plan"];
	let saved_result = (saved_plan["step"].as_u64() == Some(step as u64)
		&& saved_plan["cursor"].as_u64() == Some(cursor as u64))
	.then(|| saved_plan.get("result").cloned())
	.flatten();
	Ok((range, saved_result))
}

fn fit_workspace_read_chars(
	context: &Context,
	call: &crate::provider::ToolCall,
	output: &Value,
	budget: WorkspaceReadFitBudget,
) -> Result<Option<usize>> {
	let WorkspaceReadFitBudget {
		requested,
		offset,
		request_tokens,
		request_window,
		remaining_calls,
	} = budget;
	let reserve = remaining_calls.saturating_mul(TOOL_EVENT_RESERVE);
	let maximum_request = request_window.saturating_sub(reserve);
	let fits = |chars: usize| -> Result<bool> {
		let mut bounded_call = call.clone();
		bounded_call.arguments["max_chars"] = json!(chars);
		let bounded_output = workspace_read_result(output, requested, offset, chars);
		let event = json!({"kind":"tool","call":bounded_call,"result":bounded_output});
		Ok(
			request_tokens.saturating_add(context::tool_event_growth(context, &event))
				<= maximum_request,
		)
	};
	if !fits(0)? {
		return Ok(None);
	}
	let mut low = 0;
	let mut high = requested;
	while low < high {
		let middle = low + (high - low).div_ceil(2);
		if fits(middle)? {
			low = middle;
		} else {
			high = middle - 1;
		}
	}
	Ok(Some(low))
}

fn workspace_read_result(output: &Value, requested: usize, offset: usize, chars: usize) -> Value {
	let mut result = output.clone();
	let content: String = output["content"]
		.as_str()
		.unwrap_or_default()
		.chars()
		.take(chars)
		.collect();
	let end = offset.saturating_add(content.chars().count());
	let total = output["total_chars"].as_u64().unwrap_or(0) as usize;
	let limited = chars < requested;
	result["content"] = json!(content);
	result["next_offset"] = (end < total).then_some(json!(end)).unwrap_or(Value::Null);
	result["budget_limited"] = json!(chars == 0 || limited);
	if chars == 0 && limited && offset < total {
		result["deferred"] = json!(true);
		result["message"] = json!(
			"No request budget remains for content. Continue on a later turn; do not repeat this read now."
		);
	} else {
		if let Some(object) = result.as_object_mut() {
			object.remove("deferred");
			object.remove("message");
		}
	}
	result
}

fn skill_read_range(call: &crate::provider::ToolCall) -> Result<WorkspaceReadRange> {
	let offset = match call.arguments.get("offset") {
		None => 0,
		Some(value) => usize::try_from(value.as_u64().ok_or_else(|| {
			Error::Invalid("Skill read offset must be a nonnegative integer".into())
		})?)
		.map_err(|_| Error::Invalid("Skill read offset is too large".into()))?,
	};
	let requested = match call.arguments.get("max_chars") {
		None => 8000,
		Some(value) => usize::try_from(value.as_u64().ok_or_else(|| {
			Error::Invalid("Skill read max_chars must be a nonnegative integer".into())
		})?)
		.map_err(|_| Error::Invalid("Skill read max_chars is too large".into()))?,
	};
	if requested > 16_000 {
		return Err(Error::Invalid(
			"Skill read max_chars must not exceed 16000".into(),
		));
	}
	Ok(WorkspaceReadRange { offset, requested })
}

fn skill_read_result(output: &Value, bytes: usize) -> Value {
	let mut result = output.clone();
	let original = output["text"].as_str().unwrap_or_default();
	let end = crate::tool::bounded_utf8_end(original, 0, bytes);
	let offset = output["offset"].as_u64().unwrap_or(0) as usize;
	let total = output["total_chars"].as_u64().unwrap_or(0) as usize;
	let next = offset.saturating_add(end);
	result["text"] = json!(&original[..end]);
	result["next_offset"] = (next < total).then_some(json!(next)).unwrap_or(Value::Null);
	result["budget_limited"] = json!(end < original.len());
	if end == 0 && !original.is_empty() {
		result["deferred"] = json!(true);
		result["message"] = json!(
			"No request budget remains for this Skill file. Continue on a later turn; do not repeat this read now."
		);
	}
	result
}

fn fit_skill_read_chars(
	context: &Context,
	call: &crate::provider::ToolCall,
	output: &Value,
	budget: WorkspaceReadFitBudget,
) -> Option<usize> {
	let maximum_request = budget
		.request_window
		.saturating_sub(budget.remaining_calls.saturating_mul(TOOL_EVENT_RESERVE));
	let fits = |bytes: usize| {
		let mut bounded_call = call.clone();
		bounded_call.arguments["max_chars"] = json!(bytes);
		let event =
			json!({"kind":"tool","call":bounded_call,"result":skill_read_result(output, bytes)});
		budget
			.request_tokens
			.saturating_add(context::tool_event_growth(context, &event))
			<= maximum_request
	};
	let minimum = if budget.requested > 0 {
		output["text"]
			.as_str()
			.and_then(|text| text.chars().next())
			.map(char::len_utf8)
			.unwrap_or(0)
	} else {
		0
	};
	let mut low = minimum;
	let mut high = budget.requested.max(minimum);
	if minimum > 0 {
		if !fits(minimum) {
			return fits(0).then_some(0);
		}
	} else if !fits(0) {
		return None;
	}
	while low < high {
		let middle = low + (high - low).div_ceil(2);
		if fits(middle) {
			low = middle;
		} else {
			high = middle - 1;
		}
	}
	Some(low)
}

fn force_workspace_read_compaction_window(window: usize, minimum_request: usize) -> usize {
	window
		.saturating_sub(POST_TOOL_CONTEXT_RESERVE)
		.max(minimum_request.min(window))
}

fn deferred_workspace_read(call: &crate::provider::ToolCall) -> Value {
	json!({
		"message":"Retry this workspace_read after reducing the retained context; its result envelope did not fit.",
		"call":call
	})
}

fn deferred_skill_read(call: &crate::provider::ToolCall) -> Value {
	json!({
		"message":"Retry this skill_read after reducing the retained context; its result envelope did not fit.",
		"call":call
	})
}

fn deferred_workspace_observation(call: &crate::provider::ToolCall) -> Value {
	json!({
		"message":"Retry this workspace_observe after reducing the retained context; its page did not fit.",
		"call":call
	})
}

fn workspace_observation_event_fits(
	context: &Context,
	call: &crate::provider::ToolCall,
	limit: usize,
	output: &Value,
	request_tokens: usize,
	request_window: usize,
	remaining_calls: usize,
) -> bool {
	let mut bounded_call = call.clone();
	bounded_call.arguments["limit"] = json!(limit);
	let event = json!({"kind":"tool","call":bounded_call,"result":output});
	let maximum_request =
		request_window.saturating_sub(remaining_calls.saturating_mul(TOOL_EVENT_RESERVE));
	request_tokens.saturating_add(context::tool_event_growth(context, &event)) <= maximum_request
}

async fn run_id(store: &crate::store::Store, token: Uuid) -> Result<Uuid> {
	sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("id")),
			))
			.from(sea_orm::sea_query::Alias::new("runs"))
			.and_where(sea_orm::sea_query::Expr::cust("lease_owner = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(token)
	.fetch_optional(&store.pool)
	.await?
	.ok_or_else(|| Error::Conflict("worker lease lost".into()))
}

// Reserve the suffix before truncating, including at a UTF-8 boundary.
fn result_artifact_name(title: &str) -> String {
	let limit = 64_000 - " result".len();
	let mut end = title.len().min(limit);
	while !title.is_char_boundary(end) {
		end -= 1;
	}
	format!("{} result", &title[..end])
}

fn response_epoch(revision: i64, step: i32) -> i64 {
	revision.saturating_add(i64::from(step)).saturating_add(1)
}

#[cfg(test)]
mod review_tests {
	use serde_json::json;
	use uuid::Uuid;

	#[rstest::rstest]
	#[tokio::test(start_paused = true)]
	async fn inference_cancellation_poll_errors_do_not_signal_cancellation() {
		let pool = sqlx::postgres::PgPoolOptions::new()
			.connect_lazy("postgres://localhost/unused")
			.unwrap();
		pool.close().await;
		let store = crate::store::Store {
			capabilities: crate::capabilities::Runtime::new(Default::default()).unwrap(),
			pool: pool.clone(),
			control_pool: pool,
			node_id: "cancellation-poll-test".into(),
			semantic_client: reqwest::Client::new(),
		};
		let cancellation = super::wait_for_inference_cancellation(&store, uuid::Uuid::new_v4());
		tokio::pin!(cancellation);
		for _ in 0..3 {
			assert!(
				tokio::time::timeout(std::time::Duration::from_millis(250), &mut cancellation)
					.await
					.is_err(),
				"a failed control read must not interrupt the in-flight inference"
			);
		}
	}

	#[rstest::rstest]
	fn small_context_windows_keep_their_available_budget() {
		assert!(super::request_context_window(2048, 1500) >= 1500);
		assert!(super::request_context_window(4096, 3000) >= 3000);
		assert!(super::request_context_window(32_000, 4000) < 32_000);
	}

	#[rstest::rstest]
	fn discarded_response_keeps_a_fresh_effect_namespace_at_the_step_limit() {
		let last_step = 63;
		let old_response = super::response_epoch(20, last_step);
		let revision_after_discard = 21;
		let corrected_response = super::response_epoch(revision_after_discard, last_step);

		assert!(corrected_response > old_response);
	}

	#[rstest::rstest]
	fn uninformed_responses_may_only_read_required_unread_messages() {
		let id = Uuid::new_v4();
		let unrelated_id = Uuid::new_v4();
		let required = [id];
		let context = crate::context::Context::default();
		let read_required = crate::provider::ToolCall {
			id: "required-read".into(),
			name: "workspace_read".into(),
			arguments: json!({"kind":"message","id":id}),
		};
		assert!(super::is_required_message_read(
			&read_required,
			&required,
			&context
		));
		let unrelated_read = crate::provider::ToolCall {
			arguments: json!({"kind":"message","id":unrelated_id}),
			..read_required.clone()
		};
		assert!(!super::is_required_message_read(
			&unrelated_read,
			&required,
			&context
		));
		let mutation = crate::provider::ToolCall {
			name: "workspace_message".into(),
			arguments: json!({"content":"change the workspace"}),
			..read_required.clone()
		};
		assert!(!super::is_required_message_read(
			&mutation, &required, &context
		));
		let mut read_context = context;
		read_context.message_read_coverage.insert(
			id.to_string(),
			crate::context::MessageReadCoverage {
				total_chars: 10,
				ranges: vec![[0, 4]],
			},
		);
		assert!(!super::is_required_message_read(
			&read_required,
			&required,
			&read_context
		));
		let next_chunk = crate::provider::ToolCall {
			arguments: json!({"kind":"message","id":id,"offset":4}),
			..read_required.clone()
		};
		assert!(super::is_required_message_read(
			&next_chunk,
			&required,
			&read_context
		));
		let redundant_chunk = crate::provider::ToolCall {
			arguments: json!({"kind":"message","id":id,"offset":0}),
			..read_required
		};
		assert!(!super::is_required_message_read(
			&redundant_chunk,
			&required,
			&read_context
		));
	}

	#[rstest::rstest]
	fn tight_windows_leave_room_for_the_pinned_workspace_context() {
		for slack in [2048, 4096, 8192] {
			let minimum_request = 20_000;
			let request_window =
				super::request_context_window(minimum_request + slack, minimum_request);
			let remaining = request_window.saturating_sub(minimum_request);
			assert!(
				remaining >= crate::context::MIN_CONTEXT_RESERVE,
				"slack {slack} consumed the admission reserve"
			);

			let snapshot_budget = remaining / 4;
			let mut pinned = serde_json::json!({
				"identity":{"node_id":"node-1","agent_id":"agent-1","agent_version":"1"},
				"task":{"id":"00000000-0000-0000-0000-000000000001","title":"task"},
				"workspace":{"workspace":{"id":"00000000-0000-0000-0000-000000000002","title":"workspace"}}
			});
			crate::context::bound_snapshot(&mut pinned, snapshot_budget).unwrap();
			assert!(crate::context::estimated_tokens(&pinned.to_string()) <= snapshot_budget);
			assert_eq!(pinned["identity"]["agent_id"], "agent-1");
			assert_eq!(pinned["task"]["id"], "00000000-0000-0000-0000-000000000001");
			assert_eq!(
				pinned["workspace"]["workspace"]["id"],
				"00000000-0000-0000-0000-000000000002"
			);
		}
	}

	#[rstest::rstest]
	fn skill_read_fits_utf8_chunks_to_the_remaining_request_budget() {
		let call = crate::provider::ToolCall {
			id: "skill-1".into(),
			name: "skill_read".into(),
			arguments: serde_json::json!({"skill":{"id":"research","version":"1.0.0"},"path":"references/guide.md","offset":0,"max_chars":16000}),
		};
		let context = crate::context::Context::default();
		let output = serde_json::json!({"path":"references/guide.md","text":"界".repeat(5000),"encoding":"utf8","offset":0,"total_chars":15000,"next_offset":null});
		let full_event = serde_json::json!({"kind":"tool","call":call,"result":super::skill_read_result(&output, 16000)});
		let full_growth = crate::context::tool_event_growth(&context, &full_event);
		let minimum_event = serde_json::json!({"kind":"tool","call":call,"result":super::skill_read_result(&output, 0)});
		let minimum_growth = crate::context::tool_event_growth(&context, &minimum_event);
		assert!(full_growth > minimum_growth);
		let request_window = 100_000;
		let request_tokens = request_window - (full_growth + minimum_growth) / 2;
		let budget = super::WorkspaceReadFitBudget {
			requested: 16000,
			offset: 0,
			request_tokens,
			request_window,
			remaining_calls: 0,
		};
		let bytes = super::fit_skill_read_chars(&context, &call, &output, budget).unwrap();
		assert!(bytes > 0 && bytes < 15000);
		let result = super::skill_read_result(&output, bytes);
		let end = result["next_offset"].as_u64().unwrap() as usize;
		assert_eq!(end, result["text"].as_str().unwrap().len());
		assert_eq!(end % 3, 0);
		assert_eq!(result["budget_limited"], true);
		let mut bounded_call = call.clone();
		bounded_call.arguments["max_chars"] = serde_json::json!(bytes);
		let event = serde_json::json!({"kind":"tool","call":bounded_call,"result":result});
		assert!(
			request_tokens + crate::context::tool_event_growth(&context, &event) <= request_window
		);
		assert!(
			super::fit_skill_read_chars(
				&context,
				&call,
				&output,
				super::WorkspaceReadFitBudget {
					request_tokens: request_window,
					..budget
				}
			)
			.is_none()
		);
		assert_eq!(super::skill_read_result(&output, 0)["deferred"], true);
	}

	#[rstest::rstest]
	fn skill_read_can_fit_one_character_when_the_deferred_envelope_cannot_fit() {
		let call = crate::provider::ToolCall {
			id: "skill-1".into(),
			name: "skill_read".into(),
			arguments: serde_json::json!({"skill":{"id":"research","version":"1.0.0"},"path":"references/guide.md","offset":0,"max_chars":1}),
		};
		let context = crate::context::Context::default();
		let output = serde_json::json!({"path":"references/guide.md","text":"界more","encoding":"utf8","offset":0,"total_chars":7,"next_offset":null});
		let event_growth = |bytes| {
			let mut bounded_call = call.clone();
			bounded_call.arguments["max_chars"] = serde_json::json!(bytes);
			let event = serde_json::json!({
				"kind":"tool",
				"call":bounded_call,
				"result":super::skill_read_result(&output, bytes)
			});
			crate::context::tool_event_growth(&context, &event)
		};
		let deferred_growth = event_growth(0);
		let one_character_growth = event_growth(3);
		assert_eq!(super::skill_read_result(&output, 1)["text"], "界");
		assert!(deferred_growth > one_character_growth);

		let request_window = 10_000;
		let request_tokens = request_window - one_character_growth;
		assert!(request_tokens + deferred_growth > request_window);
		let bytes = super::fit_skill_read_chars(
			&context,
			&call,
			&output,
			super::WorkspaceReadFitBudget {
				requested: 1,
				offset: 0,
				request_tokens,
				request_window,
				remaining_calls: 0,
			},
		)
		.unwrap();
		assert_eq!(bytes, 3);
		assert_eq!(super::skill_read_result(&output, bytes)["text"], "界");
	}

	#[rstest::rstest]
	fn workspace_read_chunks_fit_remaining_complete_request_budget() {
		let call = crate::provider::ToolCall {
			id: "read-1".into(),
			name: "workspace_read".into(),
			arguments: serde_json::json!({
				"kind":"artifact",
				"id":"00000000-0000-0000-0000-000000000001",
				"offset":0,
				"max_chars":16000
			}),
		};
		let context = crate::context::Context::default();
		let record = serde_json::json!({"content":"界".repeat(20000)});
		let output = crate::context::observation::chunk_record(
			record,
			"artifact",
			"00000000-0000-0000-0000-000000000001",
			0,
			16000,
		)
		.unwrap();
		let window = 100_000;
		let request_window = super::request_context_window(window, 4_000);
		let allowed = request_window - 2 * super::TOOL_EVENT_RESERVE;
		let request_tokens = request_window - 4_000;
		let chars = super::fit_workspace_read_chars(
			&context,
			&call,
			&output,
			super::WorkspaceReadFitBudget {
				requested: 16_000,
				offset: 0,
				request_tokens,
				request_window,
				remaining_calls: 2,
			},
		)
		.unwrap()
		.unwrap();
		assert!(chars > 0 && chars < 16000);
		let mut bounded_call = call.clone();
		bounded_call.arguments["max_chars"] = serde_json::json!(chars);
		let result = super::workspace_read_result(&output, 16000, 0, chars);
		let event = serde_json::json!({"kind":"tool","call":bounded_call,"result":result});
		assert!(request_tokens + crate::context::tool_event_growth(&context, &event) <= allowed);
		let mut old_quota_call = call.clone();
		old_quota_call.arguments["max_chars"] = serde_json::json!(chars + 1);
		let old_quota_result = super::workspace_read_result(&output, 16000, 0, chars + 1);
		let old_quota_event =
			serde_json::json!({"kind":"tool","call":old_quota_call,"result":old_quota_result});
		let old_hard_window_quota =
			window - super::POST_TOOL_CONTEXT_RESERVE - 2 * super::TOOL_EVENT_RESERVE;
		assert!(
			request_tokens + crate::context::tool_event_growth(&context, &old_quota_event)
				<= old_hard_window_quota
		);
		assert!(
			request_tokens + crate::context::tool_event_growth(&context, &old_quota_event)
				> allowed
		);
		let mut too_large = call;
		too_large.arguments["max_chars"] = serde_json::json!(chars + 1);
		let result = super::workspace_read_result(&output, 16000, 0, chars + 1);
		let event = serde_json::json!({"kind":"tool","call":too_large,"result":result});
		assert!(request_tokens + crate::context::tool_event_growth(&context, &event) > allowed);
	}

	#[rstest::rstest]
	fn workspace_read_fit_uses_the_persisted_request_window() {
		let call = crate::provider::ToolCall {
			id: "read-1".into(),
			name: "workspace_read".into(),
			arguments: serde_json::json!({
				"kind":"artifact",
				"id":"00000000-0000-0000-0000-000000000001",
				"offset":0,
				"max_chars":16000
			}),
		};
		let context = crate::context::Context::default();
		let output = crate::context::observation::chunk_record(
			serde_json::json!({"content":"界".repeat(20000)}),
			"artifact",
			"00000000-0000-0000-0000-000000000001",
			0,
			16000,
		)
		.unwrap();
		let hard_window = 32_000;
		let request_window = super::request_context_window(hard_window, 8_000);
		let request_tokens = request_window - 2_000;
		let chars = super::fit_workspace_read_chars(
			&context,
			&call,
			&output,
			super::WorkspaceReadFitBudget {
				requested: 16_000,
				offset: 0,
				request_tokens,
				request_window,
				remaining_calls: 0,
			},
		)
		.unwrap()
		.expect("the persisted request quota is used without a duplicate reserve");
		assert!(chars > 0);
	}

	#[rstest::rstest]
	fn workspace_observation_fit_includes_the_adjusted_call_and_following_events() {
		let context = crate::context::Context::default();
		let call = crate::provider::ToolCall {
			id: "observe-1".into(),
			name: "workspace_observe".into(),
			arguments: serde_json::json!({"offset":0,"limit":20}),
		};
		let output = serde_json::json!({
			"view":"workspace_observation_v1",
			"tasks":[{"description_preview":"x".repeat(512),"dependencies":vec!["00000000-0000-0000-0000-000000000001";50]}],
			"pages":{"tasks":{"limit":1}}
		});
		let request_window = 32_000;
		let remaining_calls = 2;
		let target = request_window - remaining_calls * super::TOOL_EVENT_RESERVE;
		let growth = crate::context::tool_event_growth(
			&context,
			&serde_json::json!({
				"kind":"tool",
				"call":{"id":"observe-1","name":"workspace_observe","arguments":{"offset":0,"limit":1}},
				"result":output
			}),
		);
		assert!(super::workspace_observation_event_fits(
			&context,
			&call,
			1,
			&output,
			target - growth,
			request_window,
			remaining_calls
		));
		assert!(!super::workspace_observation_event_fits(
			&context,
			&call,
			1,
			&output,
			target - growth + 1,
			request_window,
			remaining_calls
		));
	}

	#[rstest::rstest]
	fn workspace_read_rejects_negative_offset_before_fitting() {
		let call = crate::provider::ToolCall {
			id: "read-1".into(),
			name: "workspace_read".into(),
			arguments: serde_json::json!({"kind":"event","id":"event-id","offset":-1}),
		};
		assert!(matches!(
			super::workspace_read_range(&call),
			Err(crate::Error::Invalid(message)) if message.contains("offset")
		));
	}

	#[rstest::rstest]
	fn workspace_read_rejects_oversized_max_chars_before_fitting() {
		let call = crate::provider::ToolCall {
			id: "read-1".into(),
			name: "workspace_read".into(),
			arguments: serde_json::json!({"kind":"event","id":"event-id","max_chars":16001}),
		};
		assert!(matches!(
			super::workspace_read_range(&call),
			Err(crate::Error::Invalid(message)) if message.contains("16000")
		));
	}

	#[rstest::rstest]
	fn cached_workspace_read_plans_still_validate_the_original_call() {
		let pending = serde_json::json!({
			"workspace_read_plan": {
				"step": 4,
				"cursor": 0,
				"result": {"content":"previously prepared"}
			}
		});
		let mut call = crate::provider::ToolCall {
			id: "read-1".into(),
			name: "workspace_read".into(),
			arguments: serde_json::json!({"kind":"event","id":"event-id","offset":-1}),
		};
		assert!(matches!(
			super::workspace_read_plan_result(&call, 4, 0, &pending),
			Err(crate::Error::Invalid(message)) if message.contains("offset")
		));

		call.arguments["offset"] = serde_json::json!(0);
		call.arguments["max_chars"] = serde_json::json!(16001);
		assert!(matches!(
			super::workspace_read_plan_result(&call, 4, 0, &pending),
			Err(crate::Error::Invalid(message)) if message.contains("16000")
		));
	}

	#[rstest::rstest]
	fn zero_length_envelope_is_checked_before_deferring_a_read() {
		let call = crate::provider::ToolCall {
			id: "read-1".into(),
			name: "workspace_read".into(),
			arguments: serde_json::json!({
				"kind":"artifact",
				"id":"00000000-0000-0000-0000-000000000001",
				"offset":0,
				"max_chars":16000
			}),
		};
		let context = crate::context::Context::default();
		let record = serde_json::json!({"content":"界".repeat(20000)});
		let output = crate::context::observation::chunk_record(
			record,
			"artifact",
			"00000000-0000-0000-0000-000000000001",
			0,
			16000,
		)
		.unwrap();
		let zero = super::workspace_read_result(&output, 16000, 0, 0);
		assert_eq!(zero["deferred"], true);
		assert!(
			super::fit_workspace_read_chars(
				&context,
				&call,
				&output,
				super::WorkspaceReadFitBudget {
					requested: 16_000,
					offset: 0,
					request_tokens: 0,
					request_window: 1,
					remaining_calls: 0,
				},
			)
			.unwrap()
			.is_none()
		);
		let next_window = super::force_workspace_read_compaction_window(32_000, 8_000);
		assert_eq!(next_window, 32_000 - super::POST_TOOL_CONTEXT_RESERVE);
		assert!(next_window < 32_000);
		let deferred = super::deferred_workspace_read(&call);
		assert_eq!(deferred["call"]["arguments"]["offset"], 0);
		assert_eq!(deferred["call"]["arguments"]["id"], call.arguments["id"]);
	}

	#[rstest::rstest]
	fn referenced_run_message_requires_every_record_chunk() {
		let id = uuid::Uuid::new_v4();
		let event = |offset: usize, content: &str, next: Option<usize>| {
			serde_json::json!({
				"kind":"tool",
				"call":{"name":"workspace_read","arguments":{"kind":"message","id":id}},
				"result":{"kind":"message","id":id,"encoding":"json","offset":offset,"total_chars":6,"content":content,"next_offset":next}
			})
		};
		let mut context = crate::context::Context::default();
		context.history.push(event(0, "abc", Some(3)));
		super::capture_message_read_coverage(&mut context);
		assert!(!super::referenced_message_read(&context, id));
		context.history.push(event(4, "ef", None));
		super::capture_message_read_coverage(&mut context);
		assert!(!super::referenced_message_read(&context, id));
		context.history.push(event(3, "def", None));
		super::capture_message_read_coverage(&mut context);
		assert!(super::referenced_message_read(&context, id));
		context.history.clear();
		assert!(super::referenced_message_read(&context, id));
		// Completed reads alone are not evidence that compaction left their
		// content in a provider request.
		super::capture_message_inference_coverage(&mut context);
		assert!(!super::referenced_message_inferred(&context, id));
		context.history.push(event(0, "abc", Some(3)));
		super::capture_message_inference_coverage(&mut context);
		assert!(!super::referenced_message_inferred(&context, id));
		context.history.clear();
		context.history.push(event(3, "def", None));
		super::capture_message_inference_coverage(&mut context);
		assert!(super::referenced_message_inferred(&context, id));
		context.history.clear();
		assert!(super::referenced_message_inferred(&context, id));
	}

	#[rstest::rstest]
	fn result_names_fit_for_ascii_and_multibyte_titles() {
		for title in ["a".repeat(64_000), "界".repeat(21_333)] {
			let name = super::result_artifact_name(&title);
			assert!(name.len() <= 64_000);
			assert!(name.ends_with(" result"));
		}
	}
}
