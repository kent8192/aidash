use crate::{
	Error, Result,
	authorization::execution::{self, Guard},
	context::{self, Context, ContextEvent, ContextUsage},
	domain::*,
	federation::{Federation, Home},
	provider::provider,
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

fn message_read_range(event: &ContextEvent) -> Option<(Uuid, usize, usize, usize)> {
	let ContextEvent::Tool {
		call,
		result: output,
	} = event
	else {
		return None;
	};
	if call.name != "workspace_read"
		|| call.arguments["kind"] != "message"
		|| output["kind"] != "message"
		|| output["encoding"] != "json"
		|| call.arguments["id"] != output["id"]
	{
		return None;
	}
	let id = output["id"].as_str()?.parse().ok()?;
	let start = output["offset"].as_u64()? as usize;
	let total = output["total_chars"].as_u64()? as usize;
	let content = output["content"].as_str()?;
	let end = start.checked_add(content.chars().count())?;
	let next = output["next_offset"].as_u64().map(|value| value as usize);
	(end <= total && end > start && next.unwrap_or(total) == end).then_some((id, start, end, total))
}

fn record_message_read_in(
	coverage_by_id: &mut BTreeMap<Uuid, context::MessageReadCoverage>,
	event: &ContextEvent,
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

fn record_message_read(context: &mut Context, event: &ContextEvent) {
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
	coverage_by_id: &BTreeMap<Uuid, context::MessageReadCoverage>,
	id: Uuid,
) -> bool {
	coverage_by_id.get(&id).is_some_and(|coverage| {
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
		.get(&id)
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

// Framework-owned transition fields are parsed once at the tool boundary.
// Other result content remains arbitrary JSON inside a typed history envelope.
enum FrameworkResult {
	Ordinary,
	Approval(Uuid),
	Human(Uuid),
	Wait(i64),
}
impl FrameworkResult {
	fn decode(call: &crate::provider::ToolCall, output: &Value) -> Result<Self> {
		#[derive(serde::Deserialize)]
		struct ApprovalView {
			approval_id: Uuid,
		}
		#[derive(serde::Deserialize)]
		struct HumanView {
			human_request_id: Option<Uuid>,
		}
		#[derive(serde::Deserialize)]
		struct WaitView {
			wait_seconds: Option<i64>,
		}
		#[derive(serde::Deserialize)]
		#[serde(rename_all = "snake_case")]
		enum FrameworkStatus {
			ApprovalRequired,
			#[serde(other)]
			Other,
		}
		let status = output
			.get("status")
			.cloned()
			.and_then(|value| serde_json::from_value::<FrameworkStatus>(value).ok());
		if matches!(status, Some(FrameworkStatus::ApprovalRequired)) {
			let view: ApprovalView = serde_json::from_value(output.clone())
				.map_err(|_| Error::Invalid("approval result is missing a valid ID".into()))?;
			return Ok(Self::Approval(view.approval_id));
		}
		match call.name.as_str() {
			"human_request" => {
				let view: HumanView = serde_json::from_value(output.clone())?;
				Ok(view.human_request_id.map_or(Self::Ordinary, Self::Human))
			}
			"workspace_wait" => {
				let view: WaitView = serde_json::from_value(output.clone())?;
				Ok(view.wait_seconds.map_or(Self::Ordinary, Self::Wait))
			}
			_ => Ok(Self::Ordinary),
		}
	}
}

#[derive(Clone)]
pub struct Harness {
	pub federation: Federation,
}
impl Harness {
	pub async fn worker_once(&self) -> Result<bool> {
		let store = &self.federation.store;
		if self.deliver_failure_once().await? {
			return Ok(true);
		}
		// Terminal runs are no longer leased, but their accepted remote inputs
		// remain in the durable outbox until home delivery is acknowledged.
		if let Some(run) = store.pending_terminal_run_message().await? {
			match self.federation.deliver_run_message_metadata(&run).await {
				Ok(()) => return Ok(true),
				Err(error) => {
					tracing::warn!(run_id=%run.id, %error, "terminal run message delivery deferred");
					store.defer_run_message_delivery(run.id).await?;
				}
			}
		}
		let visibility = crate::transactions::gate::ReadLease::begin(store).await?;
		let token = Uuid::new_v4();
		let Some(run) = store
			.lease_run(token, self.federation.config.lease_seconds)
			.await?
		else {
			return Ok(false);
		};
		self.advance_leased(run, token, visibility).await
	}
	/// The caller has committed the lease and, for notifications, its disposition.
	pub(crate) async fn advance_leased(
		&self,
		mut run: Run,
		token: Uuid,
		mut visibility: crate::transactions::gate::ReadLease,
	) -> Result<bool> {
		let store = &self.federation.store;
		let _active = crate::http::ActiveExecution::begin();
		let result = {
			let mut work = Box::pin(self.advance(&mut run, token, &mut visibility));
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
		metrics::counter!("aidash_worker_steps_total", "outcome" => if result.is_ok() { "success" } else { "error" }).increment(1);
		if let Err(e) = result {
			let id = match run_id(store, token).await {
				Ok(id) => id,
				Err(_) => return Ok(true),
			};
			let mut current = store.run(id).await?;
			let attempts = current.recovery.retry.as_ref().map_or(0, |r| r.count) + 1;
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
					.pause_for_execution(&current, token, reason, "run.authorization_blocked")
					.await?;
			} else if matches!(e, Error::MediaRouteUnavailable(_)) {
				store
					.pause_for_execution(&current, token, &e.to_string(), "run.media_route_blocked")
					.await?;
			} else if matches!(e, Error::TransactionPending | Error::StaleInference) {
				current.recovery.retry = Some(RetryState {
					count: attempts,
					at: chrono::Utc::now() + chrono::Duration::seconds(1),
				});
				store.save_run(&current, token, "run.retrying").await?;
				metrics::counter!("aidash_worker_retries_total").increment(1);
			} else if current.state.failure_delivery() {
				// Delivery is durable and unbounded; never retry the failed tool
				// just because its home node has not acknowledged terminal state.
				if let RunState::Waiting(wait) = &mut current.state
					&& let WaitingState::FailureDelivery {
						wake_at,
						last_delivery_error,
						..
					} = wait.as_mut()
				{
					*last_delivery_error = Some(e.to_string());
					*wake_at = chrono::Utc::now() + chrono::Duration::seconds(5);
				}
				store
					.save_run(&current, token, "run.failure_pending")
					.await?;
			} else if retryable_inference_error(&e)
				&& attempts <= 5
				&& current.control != RunControl::Cancelled
			{
				current.recovery.retry = Some(RetryState {
					count: attempts,
					at: chrono::Utc::now() + chrono::Duration::seconds(2_i64.pow(attempts)),
				});
				current.error = Some(e.to_string());
				store.save_run(&current, token, "run.retrying").await?;
				metrics::counter!("aidash_worker_retries_total").increment(1);
			} else {
				let target = if current.control == RunControl::Cancelled {
					FailureTarget::Cancelled
				} else {
					FailureTarget::Failed
				};
				current.state = RunState::Waiting(Box::new(WaitingState::FailureDelivery {
					target,
					wake_at: chrono::Utc::now(),
					last_delivery_error: None,
				}));
				current.error = Some(e.to_string());
				store
					.save_run(&current, token, "run.failure_pending")
					.await?;
			}
		}
		Ok(true)
	}
	async fn deliver_failure_once(&self) -> Result<bool> {
		let store = &self.federation.store;
		let token = Uuid::new_v4();
		let _visibility = crate::transactions::gate::ReadLease::begin(store).await?;
		let Some(delivery) = store
			.claim_failure_delivery(token, self.federation.config.lease_seconds)
			.await?
		else {
			return Ok(false);
		};
		if delivery.metadata.control == RunControl::Cancelled
			&& crate::authorization::peer::admission::run_grant(store, &delivery.metadata)
				.await?
				.is_some()
		{
			store
				.finish_failure_delivery(&delivery, token, Ok(TaskStatus::Cancelled))
				.await?;
			return Ok(true);
		}
		let work = async {
			let guard =
				execution::DeliveryGuard::begin(&self.federation, &delivery.metadata).await?;
			let result = async {
				self.federation
					.deliver_run_message_metadata(&delivery.metadata)
					.await?;
				let home = Home::for_delivery(self.federation.clone(), delivery.metadata.clone())
					.with_authority(
						guard
							.as_ref()
							.and_then(execution::DeliveryGuard::local_authority),
					);
				let task = home.task().await?;
				if task.status.is_terminal() {
					return Ok(task.status);
				}
				Ok(self
					.federation
					.transition_terminal_metadata(
						&delivery.metadata,
						delivery.target.task_status(),
						guard
							.as_ref()
							.and_then(execution::DeliveryGuard::local_authority),
					)
					.await?
					.status)
			}
			.await;
			if let Some(guard) = guard {
				guard.finish(result).await
			} else {
				result
			}
		};
		let result = {
			let mut work = Box::pin(work);
			let mut heartbeat = tokio::time::interval(Duration::from_secs(
				(self.federation.config.lease_seconds / 3).max(1) as u64,
			));
			heartbeat.tick().await;
			loop {
				tokio::select! {
					result = &mut work => break result,
					_ = heartbeat.tick() => {
						if !renew_worker_lease(store, delivery.metadata.id, token, self.federation.config.lease_seconds).await? { return Ok(true); }
					}
				}
			}
		};
		if let Err(
			error @ (Error::Forbidden | Error::Unauthorized | Error::IdentityStatusUnavailable),
		) = &result
		{
			store
				.pause_for_execution(
					&delivery.metadata,
					token,
					if matches!(error, Error::IdentityStatusUnavailable) {
						"identity status unavailable"
					} else {
						"execution authority denied"
					},
					"run.authorization_blocked",
				)
				.await?;
			return Ok(true);
		}
		store
			.finish_failure_delivery(&delivery, token, result)
			.await?;
		Ok(true)
	}
	/// Terminal input delivery remains independent of runnable-Run activation.
	pub async fn deliver_terminal_messages_until(
		&self,
		mut stopping: tokio::sync::watch::Receiver<bool>,
	) -> Result<()> {
		while !*stopping.borrow() && stopping.has_changed().is_ok() {
			let delivery = async {
				if self.deliver_failure_once().await? {
					return Ok(true);
				}
				if let Some(run) = self.federation.store.pending_terminal_run_message().await? {
					if self
						.federation
						.deliver_run_message_metadata(&run)
						.await
						.is_ok()
					{
						return Ok(true);
					}
					self.federation
						.store
						.defer_run_message_delivery(run.id)
						.await?;
				}
				Ok::<bool, Error>(false)
			}
			.await;
			match delivery {
				Ok(true) => continue,
				Ok(false) => {}
				Err(error) => tracing::warn!(%error, "terminal remote input delivery deferred"),
			}
			tokio::select! {
				_ = tokio::time::sleep(Duration::from_millis(250)) => {},
				_ = crate::lifecycle::stopped(&mut stopping) => break,
			}
		}
		Ok(())
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
		let runtime = crate::activation::Runtime::new(
			self.federation.clone(),
			crate::activation::Settings::from_env()?,
			true,
		);
		let mut background = tokio::task::JoinSet::new();
		background.spawn(runtime.clone().run(stopping.clone()));
		let delivery = self.clone();
		let delivery_stopping = stopping.clone();
		background.spawn(async move {
			delivery
				.deliver_terminal_messages_until(delivery_stopping)
				.await
		});
		let result = runtime.worker(self.clone(), stopping).await;
		background.abort_all();
		while background.join_next().await.is_some() {}
		result
	}

	async fn tool_error(
		&self,
		run: &mut Run,
		token: Uuid,
		call: &crate::provider::ToolCall,
		cursor: usize,
		message: String,
	) -> Result<()> {
		let mut context = run.context.clone();
		let event = ContextEvent::tool(call.clone(), json!({"error":message}));
		let growth = context::tool_event_growth(&context, &event);
		context.history.push(event);
		run.state.tool_mut()?.request_tokens =
			run.state.tool()?.request_tokens.saturating_add(growth);
		run.context = context.clone();
		run.state.tool_mut()?.cursor = cursor + 1;
		self.federation
			.store
			.save_run(run, token, "run.tool_recorded")
			.await?;
		Ok(())
	}

	async fn tool_invocation_error(
		&self,
		run: &mut Run,
		token: Uuid,
		call: &crate::provider::ToolCall,
		cursor: usize,
		unfinished_key: Option<&str>,
		message: String,
	) -> Result<()> {
		if let Some(key) = unfinished_key {
			self.federation
				.store
				.invocation_finish(run, token, key, &json!({"error":message}))
				.await?;
		}
		self.tool_error(run, token, call, cursor, message).await
	}

	async fn tools(&self, config: &AgentConfig) -> Result<BTreeMap<String, Arc<dyn Tool>>> {
		let mut tools = builtins();
		crate::capabilities::tools::add(&mut tools, &config.core_capabilities);
		tools.retain(|name, _| config.permits_builtin(name));
		for (index, reference) in config.tools.iter().enumerate() {
			let entry = self
				.federation
				.registry
				.get(&reference.id, &reference.version)
				.await?;
			let cfg: ToolConfig = serde_json::from_value(entry.config.clone())?;
			if matches!(cfg, ToolConfig::Agent { .. })
				&& config.allow_task_delegation == Some(false)
			{
				continue;
			}
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
			run.included_input_seq(),
			&format!("{}:{}:output", run.id, run.state.tool()?.response_epoch),
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
		if task.status.is_terminal() {
			run.state = match task.status {
				TaskStatus::Completed => RunState::Completed(TerminalState {}),
				TaskStatus::Failed => RunState::Failed(TerminalState {}),
				TaskStatus::Cancelled | TaskStatus::Abandoned => {
					RunState::Cancelled(TerminalState {})
				}
				TaskStatus::Open
				| TaskStatus::Claimed
				| TaskStatus::Running
				| TaskStatus::Blocked => {
					return Err(Error::Conflict("task is not terminal".into()));
				}
			};
			let kind = if run.phase() == RunPhase::Failed {
				"run.failed"
			} else {
				"run.reconciled"
			};
			store.save_run(run, token, kind).await?;
			return Ok(());
		}
		if let RunState::Waiting(wait) = &run.state
			&& let WaitingState::FailureDelivery { target, .. } = wait.as_ref()
		{
			let target = *target;
			self.federation
				.transition_terminal_run_messages(run, target.task_status())
				.await?;
			run.state = target.into_state();
			store
				.save_run(
					run,
					token,
					if target == FailureTarget::Failed {
						"run.failed"
					} else {
						"run.cancelled"
					},
				)
				.await?;
			return Ok(());
		}
		run.recovery.retry = None;
		if std::mem::take(&mut run.recovery.lease_recovered) {
			let data = json!({"run_id":run.id,"task_id":run.task_id,"phase":run.phase(),"cause":"expired worker lease"});
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
		if run.control == RunControl::Cancelled {
			self.federation
				.transition_terminal_run_messages(run, TaskStatus::Cancelled)
				.await?;
			run.state = RunState::Cancelled(TerminalState {});
			store.save_run(run, token, "run.cancelled").await?;
			return Ok(());
		}
		let entry = self
			.federation
			.registry
			.get(&run.agent_id, &run.agent_version)
			.await?;
		let agent: AgentConfig = serde_json::from_value(entry.config.clone())?;
		match run.state.clone() {
			RunState::Ready(_) => {
				let task = home.task().await?;
				if !task.dependencies.is_empty() {
					let mut waiting = false;
					for id in &task.dependencies {
						let dependency: Task = serde_json::from_value(
							home.read_record("task", &id.to_string()).await?,
						)?;
						if matches!(
							dependency.status,
							crate::domain::TaskStatus::Failed
								| crate::domain::TaskStatus::Cancelled
								| crate::domain::TaskStatus::Abandoned
						) {
							return Err(Error::Invalid(format!(
								"dependency {} is {}",
								dependency.id, dependency.status
							)));
						}
						waiting |= dependency.status != crate::domain::TaskStatus::Completed;
					}
					if waiting {
						run.state = RunState::Waiting(Box::new(WaitingState::Dependencies {
							wake_at: chrono::Utc::now() + chrono::Duration::seconds(2),
							resume: ReadyState {},
						}));
						store.save_run(run, token, "run.waiting").await?;
						return Ok(());
					}
				}
				home.claim(&task, &entry).await?;
				home.transition(crate::domain::TaskStatus::Running).await?;
				run.state = RunState::Thinking(ThinkingState::default());
				store.save_run(run, token, "run.started").await?;
			}
			RunState::Thinking(thinking) => {
				self.federation.reconcile_run_messages(run).await?;
				// An accepted remote correction is durable even if its first home
				// delivery failed. Deliver it before building any inference request.
				self.federation.deliver_run_messages(run).await?;
				let force_read_compaction = thinking.force_workspace_read_compaction;
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
				let model = provider(self.federation.client.clone(), model_cfg.clone())?;
				let mut tools = self.tools(&agent).await?;
				if let Some(guard) = guard {
					guard.filter_core_tools(&mut tools).await?;
				}
				let task = home.task().await?;
				if task.status == crate::domain::TaskStatus::Completed {
					run.state = RunState::Completed(TerminalState {});
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
				let mut context = run.context.clone();
				capture_message_read_coverage(&mut context);
				let observation = home
					.observation(0, context::observation::DEFAULT_LIMIT)
					.await?;
				let inputs = store.run_inputs(run.id).await?;
				let input_seq = inputs
					.last()
					.map_or(run.observed_input_seq, |input| input.seq);
				let summary_seq = context.run_message_summary_seq;
				// Previous media-intake turns are retained as durable observations.
				// Repeating their text here can block a later media-bearing message.
				let intake_seq = thinking
					.media_intake_through_seq
					.unwrap_or(summary_seq)
					.max(summary_seq);
				let run_message_limit = self.federation.run_message_limit(run).await?;
				let initial_page = run_message_page(&inputs, intake_seq, run_message_limit, false)?;
				let has_unprocessed_inputs = inputs.iter().any(|input| input.seq > intake_seq);
				let run_message_catchup =
					has_unprocessed_inputs && (summary_seq > 0 || initial_page.has_more);
				let page = if run_message_catchup {
					run_message_page(&inputs, intake_seq, run_message_limit, true)?
				} else {
					initial_page
				};
				let mut batch_end_seq = page
					.entries
					.last()
					.map_or(intake_seq, |(index, _)| inputs[*index].seq);
				let mut run_messages = Vec::with_capacity(page.entries.len());
				let carried_reads = thinking.deferred_run_message_reads.clone();
				let mut page_reads = Vec::new();
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
						page_reads.push(id);
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
				let memory = if guard.is_some_and(Guard::is_remote)
					|| agent.allow_cross_conversation_memory == Some(false)
				{
					json!({})
				} else {
					store.memory(&run.metadata()).await?
				};
				let mut pinned = json!({"identity":{"node_id":self.federation.config.node_id,"agent_id":run.agent_id,"agent_version":run.agent_version},"task":task,"workspace":observation,"memory":memory,"agent_state":{"phase":run.phase(),"step":run.step}});
				if let Some(deferred_read) = &thinking.deferred_workspace_read {
					pinned["deferred_workspace_read"] = json!(deferred_read);
				}
				if let Some(deferred_read) = &thinking.deferred_skill_read {
					pinned["deferred_skill_read"] = json!(deferred_read);
				}
				if let Some(deferred_observation) = &thinking.deferred_workspace_observation {
					pinned["deferred_workspace_observation"] = json!(deferred_observation);
				}
				let mut specifications = tools
					.iter()
					.filter(|(name, _)| !run_message_catchup || name.as_str() == "workspace_read")
					.map(|(_, tool)| tool)
					.map(|t| t.specification())
					.collect::<Vec<_>>();
				let selected_media = thinking.selected_media.clone();
				let media_headroom = self.federation.run_request_headroom(run).await?;
				let mut new_messages = Vec::new();
				for (position, (index, _)) in page.entries.iter().enumerate() {
					let input = &inputs[*index];
					if input.seq > context.media_inferred_seq {
						let id = input.message_id.ok_or_else(|| {
							Error::External("run message home delivery is pending".into())
						})?;
						let headroom = media_headroom.saturating_sub(
							encoded_run_message_reservation(&run_messages[..=position]),
						);
						new_messages.push((input.seq, id, headroom));
					}
				}
				let media = Box::pin(resolve_model_input_media(
					store,
					run,
					guard,
					if run_message_catchup {
						&[]
					} else {
						&selected_media
					},
					&new_messages,
					media_headroom,
					&model_cfg,
				))
				.await?;
				if media.defer_human {
					let through = media.through_seq.ok_or_else(|| {
						Error::Invalid("run media batch cannot fit the model window".into())
					})?;
					run_messages.retain(|message| {
						message["seq"].as_i64().is_some_and(|seq| seq <= through)
					});
					page_reads.retain(|id| {
						page.entries.iter().any(|(index, reference_only)| {
							*reference_only
								&& inputs[*index].seq <= through
								&& inputs[*index].message_id == Some(*id)
						})
					});
					batch_end_seq = batch_end_seq.min(through);
				}
				let mut required_run_message_reads = carried_reads.clone();
				for id in page_reads {
					if !required_run_message_reads.contains(&id) {
						required_run_message_reads.push(id);
					}
				}
				let has_run_message_references = !required_run_message_reads.is_empty();
				if media.defer_human || media.defer_selected {
					specifications.clear();
					instructions.push_str("\nMedia intake is continuing. For this interim turn, postpone required workspace reads and the cumulative run-message summary. Preserve the user goals, constraints, and corrections in these run messages and describe the media in this request as plain text. Do not call tools or complete the task; deferred media will be provided in the next request.");
				}
				let context_window = window.saturating_sub(
					crate::provider::ModelRequest::content_parts_reservation(&media.parts),
				);
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
					window: context_window,
					instructions: &instructions,
					tools: &specifications,
					max_output_tokens: output,
				};
				let minimum_request = budget
					.request(&Context::default(), &private_context)
					.estimated_total_tokens();
				budget.window = request_context_window(context_window, minimum_request);
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
				}
				if !carried_reads.is_empty() {
					pinned["deferred_run_message_reads"] = json!(carried_reads);
				}
				if has_run_message_references {
					pinned["run_message_read_instruction"] = if media.defer_human
						|| media.defer_selected
					{
						json!(
							"These run-message records still require workspace_read after media intake completes. Do not call tools in this interim request."
						)
					} else if run_message_catchup {
						json!(
							"Read every run_messages entry with requires_workspace_read and every deferred_run_message_reads ID through workspace_read(kind=message, id=record.id or the deferred ID) before returning the updated run_message_summary. Full content remains in each workspace record."
						)
					} else {
						json!(
							"Read every run_messages entry with requires_workspace_read and every deferred_run_message_reads ID through workspace_read(kind=message, id=record.id or the deferred ID) before completing the task. Full content remains in each workspace record."
						)
					};
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
						&agent,
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
				let mut request = budget.request(&context, &pinned);
				request.content_parts = media.parts;
				crate::generation::budget::Reservation::check_request(window, &request)?;
				let request_tokens = request.estimated_total_tokens();
				let media_inferred_seq_before_response = context.media_inferred_seq;
				let observed_input_seq_before_response = run.observed_input_seq;
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
				if let Some(seq) = media.through_seq {
					context.media_inferred_seq = context.media_inferred_seq.max(seq);
				}
				context.usage = Some(ContextUsage {
					input_tokens: result.input_tokens,
					output_tokens: result.output_tokens,
					context_window: window,
					compactions: context.compactions,
				});
				run.context = context.clone();
				let references_read_at_inference = required_run_message_reads
					.iter()
					.all(|id| referenced_message_inferred(&context, *id));
				if references_read_at_inference && !run_message_catchup {
					run.observed_input_seq = if media.defer_human {
						media.through_seq.unwrap_or(run.observed_input_seq)
					} else {
						input_seq
					};
				}
				run.state = RunState::ToolCall(Box::new(ToolCallState {
					response: result,
					response_epoch: response_epoch(run.revision, run.step),
					cursor: 0,
					included_input_seq: input_seq,
					request_window: budget.window,
					request_tokens,
					finalizing: false,
					media_inferred_seq_before_response,
					observed_input_seq_before_response,
					inferred_selected_media: if run_message_catchup || media.defer_selected {
						vec![]
					} else {
						selected_media.clone()
					},
					deferred_selected_media: if run_message_catchup || media.defer_selected {
						selected_media
					} else {
						vec![]
					},
					selected_media: vec![],
					deferred_human_media: media.defer_human || media.defer_selected,
					media_inferred_through_seq: media.through_seq,
					media_intake_through_seq: intake_seq,
					required_run_message_reads,
					references_read_at_inference,
					run_message_catchup,
					run_message_summary_end_seq: batch_end_seq,
					run_message_summary_limit: run_message_limit,
					deferred_reads: ThinkingState::default(),
					workspace_read_plan: None,
					skill_read_plan: None,
					workspace_observation_plan: None,
					workbench_approval_result: None,
				}));
				run.error = None;
				store.save_run(run, token, "model.completed").await?;
			}
			RunState::ToolCall(_) => {
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
				let included_input_seq = run.included_input_seq();
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
					let mut context = run.context.clone();
					let next_pending = stale_media_pending(
						&mut context,
						run.state.tool()?,
						&mut run.observed_input_seq,
					);
					run.context = context.clone();

					run.state = RunState::Thinking(next_pending);
					store.save_run(run, token, "run.message_received").await?;
					return Ok(());
				}
				let mut result = run.state.tool()?.response.clone();
				if run.state.tool()?.deferred_human_media {
					let mut context = run.context.clone();
					if result.text.trim().is_empty() {
						let next_pending = stale_media_pending(
							&mut context,
							run.state.tool()?,
							&mut run.observed_input_seq,
						);
						run.context = context.clone();

						run.step += 1;
						run.state = RunState::Thinking(next_pending);
						store
							.save_run(run, token, "run.media_observation_required")
							.await?;
						return Ok(());
					}
					record_media_observation(
						&mut context,
						&result.text,
						run.state.tool()?.media_inferred_through_seq,
						media_observation_budget(run.state.tool()?),
					);
					run.context = context.clone();

					run.step += 1;
					let pending = run.state.tool()?;
					run.state = RunState::Thinking(ThinkingState {
						selected_media: pending.deferred_selected_media.clone(),
						media_intake_through_seq: Some(
							pending
								.media_inferred_through_seq
								.unwrap_or(pending.media_intake_through_seq),
						),
						deferred_run_message_reads: pending.required_run_message_reads.clone(),
						..Default::default()
					});
					store.save_run(run, token, "run.media_deferred").await?;
					return Ok(());
				}
				let run_message_catchup = run.state.tool()?.run_message_catchup;
				if run_message_catchup {
					// A provider response cannot execute task tools during catch-up,
					// even if it returns calls that were not in the advertised tool set.
					result
						.tool_calls
						.retain(|call| call.name == "workspace_read");
				}
				let required_reads = run.state.tool()?.required_run_message_reads.clone();
				let mut context = run.context.clone();
				capture_message_read_coverage(&mut context);
				run.context = context.clone();
				let references_read = required_reads
					.iter()
					.all(|id| referenced_message_read(&context, *id));
				let references_inferred = required_reads
					.iter()
					.all(|id| referenced_message_inferred(&context, *id));
				let informed_response = required_reads.is_empty()
					|| (references_read
						&& references_inferred
						&& run.state.tool()?.references_read_at_inference);
				let cursor = run.state.tool()?.cursor;
				if !informed_response
					&& let Some(call) = result.tool_calls.get(cursor)
					&& !is_required_message_read(call, &required_reads, &context)
				{
					context.history.push(ContextEvent::RunMessageReadRequired {
						message_ids: required_reads.clone(),
					});

					run.step += 1;
					run.state = RunState::Thinking(stale_media_pending(
						&mut context,
						run.state.tool()?,
						&mut run.observed_input_seq,
					));
					run.context = context.clone();
					store
						.save_run(run, token, "run.message_read_required")
						.await?;
					return Ok(());
				}
				if cursor >= result.tool_calls.len() && !informed_response {
					context.history.push(ContextEvent::RunMessageReadRequired {
						message_ids: required_reads.clone(),
					});

					run.step += 1;
					run.state = RunState::Thinking(stale_media_pending(
						&mut context,
						run.state.tool()?,
						&mut run.observed_input_seq,
					));
					run.context = context.clone();
					store
						.save_run(run, token, "run.message_read_required")
						.await?;
					return Ok(());
				}
				if cursor >= result.tool_calls.len() && run_message_catchup {
					let summary = result.text.trim().to_owned();
					if summary.is_empty() {
						context
							.history
							.push(ContextEvent::RunMessageSummaryRequired {
								through_seq: run.state.tool()?.run_message_summary_end_seq,
								max_bytes: None,
								reason: None,
							});

						run.step += 1;
						run.state = RunState::Thinking(stale_media_pending(
							&mut context,
							run.state.tool()?,
							&mut run.observed_input_seq,
						));
						run.context = context.clone();
						store
							.save_run(run, token, "run.message_summary_required")
							.await?;
						return Ok(());
					}
					let summary_limit = run.state.tool()?.run_message_summary_limit;
					if summary.len() > summary_limit {
						context
							.history
							.push(ContextEvent::RunMessageSummaryRequired {
								through_seq: run.state.tool()?.run_message_summary_end_seq,
								max_bytes: Some(summary_limit),
								reason: Some("summary exceeded the complete-summary limit".into()),
							});

						run.step += 1;
						run.state = RunState::Thinking(stale_media_pending(
							&mut context,
							run.state.tool()?,
							&mut run.observed_input_seq,
						));
						run.context = context.clone();
						store
							.save_run(run, token, "run.message_summary_required")
							.await?;
						return Ok(());
					}
					context.run_message_summary = summary;
					context.run_message_summary_seq = run.state.tool()?.run_message_summary_end_seq;
					let summarized_ids = required_reads
						.iter()
						.copied()
						.collect::<std::collections::BTreeSet<_>>();
					context.history.retain(|event| {
						message_read_range(event)
							.is_none_or(|(id, _, _, _)| !summarized_ids.contains(&id))
					});
					for id in &required_reads {
						context.message_read_coverage.remove(id);
						context.message_inference_coverage.remove(id);
					}
					run.context = context.clone();

					run.state = RunState::Thinking(ThinkingState {
						selected_media: pending_selected_media(run.state.tool()?),
						..Default::default()
					});
					store.save_run(run, token, "run.message_summarized").await?;
					return Ok(());
				}
				if !run_message_catchup && !result.tool_calls.is_empty() && informed_response {
					self.publish_model_text(&home, guard, run, token, &result.text)
						.await?;
				}
				if cursor >= result.tool_calls.len() {
					if result.tool_calls.is_empty() {
						if !run.state.tool()?.deferred_selected_media.is_empty() {
							if !result.text.trim().is_empty() {
								let mut context = run.context.clone();
								record_media_observation(
									&mut context,
									&result.text,
									None,
									media_observation_budget(run.state.tool()?),
								);
								run.context = context.clone();
							}

							run.step += 1;
							run.state = RunState::Thinking(ThinkingState {
								selected_media: run.state.tool()?.deferred_selected_media.clone(),
								..Default::default()
							});
							store.save_run(run, token, "run.media_deferred").await?;
							return Ok(());
						}
						let children = home.child_summary(run.task_id).await?;
						if children.has_pending {
							self.publish_model_text(&home, guard, run, token, &result.text)
								.await?;
							let failed = children.has_failed;
							if failed {
								if let Some(guard) = guard {
									guard.action("human.request", "run", run.id).await?;
								}
								let response_epoch = run.state.tool()?.response_epoch;
								let h=store.human_request(run,"INFORMATION_REQUEST","A subtask needs intervention. You can explicitly abandon failed, blocked or cancelled subtasks in their task details, providing a reason. Then answer this request to continue with the remaining results, or cancel this parent.",&format!("{}:{}:subtasks",run.id,response_epoch)).await?;
								run.state = RunState::Waiting(Box::new(WaitingState::Human {
									request_id: h.id,
									resume: ResumeState::Thinking(ThinkingState::default()),
								}));
							} else {
								run.state = RunState::Waiting(Box::new(WaitingState::Children {
									wake_at: chrono::Utc::now() + chrono::Duration::seconds(2),
									resume: ThinkingState::default(),
								}));
							}
							run.step += 1;

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
							run.state = RunState::Thinking(ThinkingState::default());

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

								run.state = RunState::Waiting(Box::new(WaitingState::Children {
									wake_at: chrono::Utc::now() + chrono::Duration::seconds(2),
									resume: ThinkingState::default(),
								}));
								store.save_run(run, token, "run.waiting").await?;
								return Ok(());
							}
							return Err(error);
						}
						run.state = RunState::Completed(TerminalState {});

						store.save_run(run, token, "run.completed").await?;
					} else {
						run.step += 1;
						let mut next = run.state.tool()?.deferred_reads.clone();
						next.selected_media = pending_selected_media(run.state.tool()?);
						run.state = RunState::Thinking(next);
						store.save_run(run, token, "run.thinking").await?;
					}
					return Ok(());
				}
				let mut context = run.context.clone();
				capture_message_read_coverage(&mut context);
				let mut call = result.tool_calls[cursor].clone();
				if !pending_selected_media(run.state.tool()?).is_empty()
					&& !read_only_after_model_media_selection(&call.name)
				{
					return self
						.tool_error(
							run,
							token,
							&call,
							cursor,
							"selected model media must be inferred before this tool call; retry it after the next model response".into(),
						)
						.await;
				}
				let mut prepared_result = None;
				if call.name == "workspace_read" {
					let (read_range, saved_read) = match workspace_read_plan_result(
						&call,
						run.step,
						cursor,
						run.state.tool()?,
					) {
						Ok(plan) => plan,
						Err(Error::Invalid(message)) => {
							return self.tool_error(run, token, &call, cursor, message).await;
						}
						Err(error) => return Err(error),
					};
					if let Some(output) = saved_read {
						prepared_result = Some(output.clone());
					} else {
						let request_tokens = run.state.tool()?.request_tokens;
						let request_window = run.state.tool()?.request_window;
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

								if !run_message_catchup {
									run.step += 1;
								}
								run.state = RunState::Thinking(ThinkingState {
									force_workspace_read_compaction: true,
									deferred_workspace_read: Some(deferred_workspace_read(&call)),
									selected_media: pending_selected_media(run.state.tool()?),
									..Default::default()
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
								run.state.tool_mut()?.response = result.clone();
								run.state.tool_mut()?.workspace_read_plan = Some(ReadPlan {
									step: run.step,
									cursor,
									call: call.clone(),
									result: output.clone(),
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
					let saved_output = run
						.state
						.tool()?
						.skill_read_plan
						.as_ref()
						.filter(|p| p.step == run.step && p.cursor == cursor && p.call == call)
						.map(|p| p.result.clone());
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
							.ok_or_else(|| Error::Invalid("skill_read builtin unavailable".into()))?
							.invoke(&ctx, call.arguments.clone(), "")
							.await
						{
							Ok(output) => output,
							Err(Error::Invalid(message)) => {
								return self.tool_error(run, token, &call, cursor, message).await;
							}
							Err(error) => return Err(error),
						};
						let request_tokens = run.state.tool()?.request_tokens;
						let request_window = run.state.tool()?.request_window;
						let budget = WorkspaceReadFitBudget {
							requested: range.requested,
							offset: range.offset,
							request_tokens,
							request_window,
							remaining_calls: result.tool_calls.len().saturating_sub(cursor + 1),
						};
						let Some(chars) = fit_skill_read_chars(&context, &call, &output, budget)
						else {
							run.step += 1;
							run.state = RunState::Thinking(ThinkingState {
								force_workspace_read_compaction: true,
								deferred_skill_read: Some(deferred_skill_read(&call)),
								selected_media: pending_selected_media(run.state.tool()?),
								..Default::default()
							});
							store
								.save_run(run, token, "run.skill_read_deferred")
								.await?;
							return Ok(());
						};
						call.arguments["max_chars"] = json!(chars);
						result.tool_calls[cursor] = call.clone();
						run.state.tool_mut()?.response = result.clone();
						let bounded = skill_read_result(&output, chars);
						run.state.tool_mut()?.skill_read_plan = Some(ReadPlan {
							step: run.step,
							cursor,
							call: call.clone(),
							result: bounded.clone(),
						});
						prepared_result = Some(bounded);
					}
				}
				if call.name == "workspace_observe" {
					let saved_output = run
						.state
						.tool()?
						.workspace_observation_plan
						.as_ref()
						.filter(|p| p.step == run.step && p.cursor == cursor && p.call == call)
						.map(|p| p.result.clone());
					if let Some(output) = saved_output {
						prepared_result = Some(output);
					} else {
						let offset = call.arguments["offset"].as_u64().unwrap_or(0) as usize;
						let requested = call.arguments["limit"]
							.as_u64()
							.unwrap_or(context::observation::DEFAULT_LIMIT as u64)
							as usize;
						let request_tokens = run.state.tool()?.request_tokens;
						let request_window = run.state.tool()?.request_window;
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

							run.step += 1;
							run.state = RunState::Thinking(ThinkingState {
								force_workspace_read_compaction: true,
								deferred_workspace_observation: Some(
									deferred_workspace_observation(&call),
								),
								selected_media: pending_selected_media(run.state.tool()?),
								..Default::default()
							});
							store
								.save_run(run, token, "run.observation_deferred")
								.await?;
							return Ok(());
						};
						call.arguments["limit"] = json!(limit);
						result.tool_calls[cursor] = call.clone();
						run.state.tool_mut()?.response = result.clone();
						run.state.tool_mut()?.workspace_observation_plan = Some(ReadPlan {
							step: run.step,
							cursor,
							call: call.clone(),
							result: output.clone(),
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
				let response_epoch = run.state.tool()?.response_epoch;
				let key = format!("{}:{}:{}", run.id, response_epoch, cursor);
				// New workbench versions require an explicit, one-call approval for
				// external writes. Legacy versions have no behavior flags and keep
				// their existing execution contract.
				if agent.allow_task_creation.is_some()
					&& let Some(index) = call
						.name
						.strip_prefix("plugin_")
						.and_then(|value| value.parse::<usize>().ok())
					&& let Some(reference) = agent.tools.get(index)
				{
					let entry = self
						.federation
						.registry
						.get(&reference.id, &reference.version)
						.await?;
					let config: ToolConfig = serde_json::from_value(entry.config)?;
					let writes = matches!(config, ToolConfig::Http { ref replay, .. } | ToolConfig::Mcp { ref replay, .. } if replay != "read_only");
					if writes {
						if let Some(decision) = run
							.state
							.tool()?
							.workbench_approval_result
							.as_ref()
							.filter(|d| d.key == key && d.call == *call)
						{
							if !decision.approved || decision.expires_at <= chrono::Utc::now() {
								return self
									.tool_error(
										run,
										token,
										call,
										cursor,
										"external write approval was denied or expired".into(),
									)
									.await;
							}
						} else {
							let prompt = format!(
								"Approve this exact external tool action once? Tool: {}@{}; call: {}",
								reference.id,
								reference.version,
								serde_json::to_string(call)?
							);
							let request = store
								.human_request(
									run,
									"APPROVAL_REQUIRED",
									&prompt,
									&format!("{key}:workbench-approval"),
								)
								.await?;
							run.state =
								RunState::Waiting(Box::new(WaitingState::ExternalApproval {
									request_id: request.id,
									key: key.clone(),
									call: call.clone(),
									expires_at: request.created_at + chrono::Duration::minutes(15),
									resume: Box::new(run.state.tool()?.clone()),
								}));
							store
								.save_run(run, token, "run.waiting_for_tool_approval")
								.await?;
							return Ok(());
						}
					}
				}
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
				let unfinished_key = (invocation.status != "COMPLETED").then_some(key.as_str());
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
				if call.name == "file_read"
					&& call.arguments["representation"] == "model_input"
					&& output["status"] == "completed"
					&& output["metadata"]["file_id"] == call.arguments["file_id"]
				{
					let selection: crate::capabilities::sharing::Selection =
						serde_json::from_value(
							json!({"file_id":call.arguments["file_id"],"expected_digest":call.arguments["expected_digest"]}),
						)?;
					let mut selected = run.state.tool()?.selected_media.clone();
					let deferred_count = run.state.tool()?.deferred_selected_media.len();
					if selected.len() + deferred_count >= 8 {
						return self
							.tool_invocation_error(
								run,
								token,
								call,
								cursor,
								unfinished_key,
								"model media input exceeds count limit".into(),
							)
							.await;
					}
					selected.push(selection);
					let mut selections = run.state.tool()?.deferred_selected_media.clone();
					selections.extend(selected.iter().cloned());
					let Some(guard) = guard else {
						return self
							.tool_invocation_error(
								run,
								token,
								call,
								cursor,
								unfinished_key,
								"model media input requires scoped file access".into(),
							)
							.await;
					};
					match Box::pin(guard.model_media(store, &selections)).await {
						Ok(parts) => {
							let headroom = self.federation.run_request_headroom(run).await?;
							let model_entry = self
								.federation
								.registry
								.get(&agent.model.id, &agent.model.version)
								.await?;
							let model: ModelConfig = serde_json::from_value(model_entry.config)?;
							match check_model_media_headroom(headroom, parts, &model) {
								Ok(()) => {}
								Err(Error::Invalid(message)) => {
									return self
										.tool_invocation_error(
											run,
											token,
											call,
											cursor,
											unfinished_key,
											message,
										)
										.await;
								}
								Err(error) => return Err(error),
							}
						}
						Err(Error::Invalid(message)) => {
							return self
								.tool_invocation_error(
									run,
									token,
									call,
									cursor,
									unfinished_key,
									message,
								)
								.await;
						}
						Err(error) => return Err(error),
					}
					run.state.tool_mut()?.selected_media = selected;
				}
				if invocation.status != "COMPLETED" {
					store.invocation_finish(run, token, &key, &output).await?;
				}
				let event = ContextEvent::tool(call.clone(), output.clone());
				record_message_read(&mut context, &event);
				let growth = context::tool_event_growth(&context, &event);
				context.history.push(event);
				run.state.tool_mut()?.request_tokens =
					run.state.tool()?.request_tokens.saturating_add(growth);
				run.context = context.clone();
				let progress = run.state.tool_mut()?;
				progress.workspace_read_plan = None;
				progress.skill_read_plan = None;
				progress.workspace_observation_plan = None;
				progress.cursor = cursor + 1;
				match FrameworkResult::decode(call, &output)? {
					FrameworkResult::Approval(approval_id) => {
						run.step += 1;
						run.state = RunState::Waiting(Box::new(WaitingState::CoreApproval {
							approval_id,
							resume: ThinkingState::default(),
						}));
					}
					FrameworkResult::Human(request_id) => {
						run.step += 1;
						run.state = RunState::Waiting(Box::new(WaitingState::Human {
							request_id,
							resume: ResumeState::Thinking(ThinkingState::default()),
						}));
					}
					FrameworkResult::Wait(seconds) => {
						run.suspend(|resume| WaitingState::Timer {
							wake_at: chrono::Utc::now() + chrono::Duration::seconds(seconds),
							resume,
						})?
					}
					FrameworkResult::Ordinary => {}
				}
				home.report(
					&format!("{key}:tool"),
					"remote.tool.completed",
					json!({"run_id":run.id,"call":call,"result":output}),
				)
				.await?;
				store.save_run(run, token, "run.tool_recorded").await?;
			}
			RunState::Waiting(waiting) => {
				let mut waiting = *waiting;
				if let Some(id) = waiting.request_id() {
					if let Some(guard) = guard {
						guard.human_read(id).await?;
					}
					let h: HumanRequest = sqlx::query_as(
						&sea_orm::sea_query::Query::select()
							.column(sea_orm::sea_query::Asterisk)
							.from(sea_orm::sea_query::Alias::new("human_requests"))
							.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
							.to_string(sea_orm::sea_query::PostgresQueryBuilder),
					)
					.bind(id)
					.fetch_one(&store.pool)
					.await?;
					let response = if matches!(&waiting, WaitingState::ExternalApproval {expires_at,..} if *expires_at <= chrono::Utc::now()) {store.expire_workbench_approval(id).await?.response} else {h.response}.ok_or_else(||Error::Conflict("human request has not been answered".into()))?;
					match &mut waiting {
						WaitingState::ExternalApproval {
							key,
							call,
							expires_at,
							resume,
							..
						} => {
							#[derive(serde::Deserialize)]
							struct ApprovalAnswer {
								approved: bool,
							}
							let decision =
								serde_json::from_value::<ApprovalAnswer>(response.clone()).ok();
							resume.workbench_approval_result = Some(ApprovalDecision {
								key: key.clone(),
								call: call.clone(),
								approved: decision.is_some_and(|d| d.approved),
								expires_at: *expires_at,
							})
						}
						WaitingState::Reconciliation { key, .. } => {
							#[derive(serde::Deserialize)]
							struct ReconciliationAnswer {
								#[serde(deserialize_with = "crate::domain::required_json")]
								result: Value,
							}
							let answer: ReconciliationAnswer =
								serde_json::from_value(response.clone()).map_err(|_| {
									Error::Invalid(
										"reconciliation response must contain result".into(),
									)
								})?;
							store
								.invocation_finish(run, token, key, &answer.result)
								.await?;
						}
						_ => {}
					}
					if !matches!(waiting, WaitingState::Reconciliation { .. }) {
						run.context.history.push(ContextEvent::Human {
							request: h.prompt,
							request_kind: h.kind,
							response,
						});
					}
				}
				run.state = match waiting {
					WaitingState::Dependencies { resume, .. } => RunState::Ready(resume),
					WaitingState::Children { resume, .. }
					| WaitingState::CoreApproval { resume, .. } => RunState::Thinking(resume),
					WaitingState::Timer { resume, .. } | WaitingState::Human { resume, .. } => {
						resume.into_state()
					}
					WaitingState::ExternalApproval { resume, .. }
					| WaitingState::Reconciliation { resume, .. } => RunState::ToolCall(resume),
					WaitingState::FailureDelivery { .. } => {
						return Err(Error::Invalid(
							"failure delivery requires terminal handling".into(),
						));
					}
				};
				store.save_run(run, token, "run.resumed").await?;
			}
			RunState::Completed(_) | RunState::Failed(_) | RunState::Cancelled(_) => {
				return Err(Error::Conflict("run is not executable".into()));
			}
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
		let control = sqlx::query_scalar::<_, RunControl>(&query)
			.bind(id)
			.fetch_one(&store.pool)
			.await;
		match control {
			Ok(RunControl::Cancelled) => return Ok(()),
			Ok(RunControl::Active | RunControl::Paused) => {}
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
	pending: &ToolCallState,
) -> Result<(WorkspaceReadRange, Option<Value>)> {
	let range = workspace_read_range(call)?;
	let saved_result = pending
		.workspace_read_plan
		.as_ref()
		.filter(|p| p.step == step && p.cursor == cursor && &p.call == call)
		.map(|p| p.result.clone());
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
		let event = ContextEvent::tool(bounded_call, bounded_output);
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
		let event = ContextEvent::tool(bounded_call, skill_read_result(output, bytes));
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

fn deferred_workspace_read(call: &crate::provider::ToolCall) -> Box<DeferredRead> {
	Box::new(DeferredRead {
		message: "Retry this workspace_read after reducing the retained context; its result envelope did not fit.".into(),
		call: call.clone(),
	})
}

fn deferred_skill_read(call: &crate::provider::ToolCall) -> Box<DeferredRead> {
	Box::new(DeferredRead {
		message: "Retry this skill_read after reducing the retained context; its result envelope did not fit.".into(),
		call: call.clone(),
	})
}

fn deferred_workspace_observation(call: &crate::provider::ToolCall) -> Box<DeferredRead> {
	Box::new(DeferredRead {
		message: "Retry this workspace_observe after reducing the retained context; its page did not fit.".into(),
		call: call.clone(),
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
	let event = ContextEvent::tool(bounded_call, output.clone());
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

struct ResolvedMedia {
	parts: Vec<crate::provider::ContentPart>,
	through_seq: Option<i64>,
	defer_selected: bool,
	defer_human: bool,
}

async fn resolve_model_input_media(
	store: &crate::store::Store,
	run: &Run,
	guard: Option<&Guard>,
	selections: &[crate::capabilities::sharing::Selection],
	messages: &[(i64, Uuid, usize)],
	headroom: usize,
	model: &ModelConfig,
) -> Result<ResolvedMedia> {
	let selected_parts = if selections.is_empty() {
		Vec::new()
	} else {
		guard
			.ok_or(Error::Forbidden)?
			.model_media(store, selections)
			.await?
	};
	let has_human_media = store
		.run_message_has_media(&messages.iter().map(|(_, id, _)| *id).collect::<Vec<_>>())
		.await?;
	let human = if !has_human_media {
		crate::authorization::execution::HumanMediaBatch {
			parts: Vec::new(),
			through_seq: messages.last().map(|(seq, _, _)| *seq),
			has_more: false,
		}
	} else if let Some(guard) = guard {
		guard.human_message_media(messages, model).await?
	} else {
		crate::authorization::execution::operator_human_message_media(store, run, messages, model)
			.await?
	};
	let batch_headroom = human
		.through_seq
		.and_then(|through| {
			messages
				.iter()
				.find(|(seq, _, _)| *seq == through)
				.map(|(_, _, available)| *available)
		})
		.unwrap_or(headroom);
	let (parts, defer_selected) = choose_inference_media(
		selected_parts,
		human.parts,
		batch_headroom,
		human.through_seq.is_some(),
		model,
	);
	if !model.has_current_media_route_for_parts(&parts) {
		return Err(Error::MediaRouteUnavailable(model.model_id.clone()));
	}
	Ok(ResolvedMedia {
		parts,
		through_seq: human.through_seq,
		defer_selected,
		defer_human: human.has_more,
	})
}

fn choose_inference_media(
	selected_parts: Vec<crate::provider::ContentPart>,
	human_parts: Vec<crate::provider::ContentPart>,
	headroom: usize,
	human_batch_present: bool,
	model: &ModelConfig,
) -> (Vec<crate::provider::ContentPart>, bool) {
	let mut combined = selected_parts;
	combined.extend(human_parts.iter().cloned());
	let defer_selected = human_batch_present
		&& (!crate::provider::ModelRequest::media_within_limits(&combined)
			|| media_request_headroom(headroom, &combined).is_err()
			|| !model.has_current_media_route_for_parts(&combined));
	(
		if defer_selected {
			human_parts
		} else {
			combined
		},
		defer_selected,
	)
}

fn media_request_headroom(headroom: usize, parts: &[crate::provider::ContentPart]) -> Result<()> {
	let request = crate::provider::ModelRequest {
		instructions: String::new(),
		context: json!({}),
		tools: Vec::new(),
		max_output_tokens: 0,
		content_parts: Vec::new(),
	};
	crate::generation::budget::Reservation::check_request_with_parts(headroom, &request, parts)
}

fn encoded_run_message_reservation(messages: &[Value]) -> usize {
	if messages.is_empty() {
		return 0;
	}
	let estimate = |current: Value| {
		crate::provider::ModelRequest {
			instructions: String::new(),
			context: json!({
				"current":current,
				"summary":"",
				"run_message_summary":"",
				"history":[]
			}),
			tools: Vec::new(),
			max_output_tokens: 0,
			content_parts: Vec::new(),
		}
		.estimated_total_tokens()
	};
	estimate(json!({"run_messages":messages})).saturating_sub(estimate(json!({})))
}

fn read_only_after_model_media_selection(name: &str) -> bool {
	matches!(
		name,
		"file_read"
			| "file_search"
			| "workspace_read"
			| "workspace_observe"
			| "skill_list"
			| "skill_read"
	)
}

fn pending_selected_media(pending: &ToolCallState) -> Vec<crate::capabilities::sharing::Selection> {
	pending.selected_media()
}

fn check_model_media_headroom(
	headroom: usize,
	parts: Vec<crate::provider::ContentPart>,
	model: &ModelConfig,
) -> Result<()> {
	let request = crate::provider::ModelRequest {
		instructions: String::new(),
		context: json!({}),
		tools: Vec::new(),
		max_output_tokens: 0,
		content_parts: parts,
	};
	request.validate()?;
	if !model.has_current_media_route_for_parts(&request.content_parts) {
		return Err(Error::Invalid(format!(
			"recipient model {} has no current media route for every selected format",
			model.model_id
		)));
	}
	crate::generation::budget::Reservation::check_request(headroom, &request)
}

fn media_observation_budget(pending: &ToolCallState) -> usize {
	(pending.request_window / 16).clamp(256, 4096)
}

fn record_media_observation(
	context: &mut Context,
	text: &str,
	through_seq: Option<i64>,
	budget: usize,
) {
	let source = text.trim();
	let mut end = source.len().min(1024);
	while !source.is_char_boundary(end) {
		end -= 1;
	}
	loop {
		let event = ContextEvent::ModelMediaObservation {
			text: source[..end].into(),
			through_seq,
			truncated: end < source.len(),
		};
		if event.encoded_len() <= budget {
			context.history.push(event);
			break;
		}
		end -= 1;
		while !source.is_char_boundary(end) {
			end -= 1;
		}
	}
	let mut used = 0_usize;
	let mut remove = Vec::new();
	for index in (0..context.history.len()).rev() {
		let event = &context.history[index];
		if matches!(event, ContextEvent::ModelMediaObservation { .. }) {
			let bytes = event.to_string().len();
			if used.saturating_add(bytes) > budget {
				remove.push(index);
			} else {
				used += bytes;
			}
		}
	}
	for index in remove {
		context.history.remove(index);
	}
}

fn stale_media_pending(
	context: &mut Context,
	pending: &ToolCallState,
	observed_input_seq: &mut i64,
) -> ThinkingState {
	pending.stale(context, observed_input_seq)
}

fn response_epoch(revision: i64, step: i32) -> i64 {
	revision.saturating_add(i64::from(step)).saturating_add(1)
}

fn retryable_inference_error(error: &Error) -> bool {
	matches!(error, Error::External(_))
		|| matches!(
			error,
			Error::ProviderRejected {
				status: 408 | 429 | 500..=599,
				..
			}
		)
}

#[cfg(test)]
mod review_tests {
	use crate::Error;
	use crate::context::ContextEvent;
	fn typed_event(value: serde_json::Value) -> ContextEvent {
		serde_json::from_value(value).unwrap()
	}
	fn pending(value: serde_json::Value) -> crate::domain::ToolCallState {
		let mut complete = serde_json::to_value(crate::domain::ToolCallState::default()).unwrap();
		for (key, value) in value.as_object().unwrap() {
			if key == "response" {
				let mut response = json!(crate::provider::ModelResponse::default());
				for (name, value) in value.as_object().unwrap() {
					response[name] = value.clone();
				}
				complete[key] = response;
			} else {
				complete[key] = value.clone();
			}
		}
		serde_json::from_value(complete).unwrap()
	}
	use serde_json::json;
	use uuid::Uuid;

	fn media_model() -> crate::registry::ModelConfig {
		serde_json::from_value(json!({
			"provider":"openrouter", "model_id":"fixture", "endpoint":"https://example.com",
			"credential_env":null, "context_window":128000, "max_output_tokens":4096,
			"modalities":["text","image","audio"], "cost":{},
			"media_routes":[
				{"tag":"fixture/png", "formats":["image/png", "wav"],
				"source":"test", "verified_at":chrono::Utc::now() - chrono::Duration::hours(1),
				"expires_at":chrono::Utc::now() + chrono::Duration::hours(1)},
				{"tag":"fixture/jpeg", "formats":["image/jpeg"],
				"source":"test", "verified_at":chrono::Utc::now() - chrono::Duration::hours(1),
				"expires_at":chrono::Utc::now() + chrono::Duration::hours(1)}
			]
		}))
		.unwrap()
	}

	#[rstest::rstest]
	fn transient_provider_statuses_keep_the_worker_retry_path() {
		for status in [408, 429, 500, 503] {
			assert!(super::retryable_inference_error(&Error::ProviderRejected {
				status,
				reason: "upstream rejected the request".into(),
			}));
		}
		for status in [400, 401, 403, 413] {
			assert!(!super::retryable_inference_error(
				&Error::ProviderRejected {
					status,
					reason: "upstream rejected the request".into(),
				}
			));
		}
	}

	#[rstest::rstest]
	fn stale_response_requeues_media_and_restores_its_inference_marker() {
		let first = json!({"file_id":Uuid::new_v4(),"expected_digest":"one"});
		let second = json!({"file_id":Uuid::new_v4(),"expected_digest":"two"});
		let mut context = crate::context::Context {
			media_inferred_seq: 5,
			..Default::default()
		};
		let mut observed_input_seq = 5;
		let required = Uuid::new_v4();
		let pending = pending(json!({
			"media_inferred_seq_before_response":3,
			"observed_input_seq_before_response":3,
			"media_intake_through_seq":2,
			"required_run_message_reads":[required],
			"inferred_selected_media":[first],
			"selected_media":[first,second]
		}));
		let next = super::stale_media_pending(&mut context, &pending, &mut observed_input_seq);
		assert_eq!(context.media_inferred_seq, 3);
		assert_eq!(observed_input_seq, 3);
		assert_eq!(json!(next.selected_media), json!([first, second]));
		assert_eq!(next.media_intake_through_seq, Some(2));
		assert_eq!(json!(next.deferred_run_message_reads), json!([required]));
	}

	#[test]
	fn durable_media_observations_keep_recent_text_within_the_request_budget() {
		let mut context = crate::context::Context {
			history: vec![crate::context::ContextEvent::tool(
				crate::provider::ToolCall {
					id: "keep".into(),
					name: "read".into(),
					arguments: json!({}),
				},
				json!("keep"),
			)],
			..Default::default()
		};
		let budget = super::media_observation_budget(&pending(json!({"request_window":8192})));
		assert_eq!(budget, 512);
		for seq in 1..=20 {
			super::record_media_observation(
				&mut context,
				&"あ\\\"".repeat(1000),
				Some(seq),
				budget,
			);
		}
		let observations: Vec<_> = context
			.history
			.iter()
			.filter(|event| matches!(event, ContextEvent::ModelMediaObservation { .. }))
			.collect();
		assert!(observations.len() < 20);
		assert_eq!(json!(observations.last().unwrap())["through_seq"], 20);
		assert_eq!(json!(observations.last().unwrap())["truncated"], true);
		assert!(
			observations
				.iter()
				.map(|event| event.to_string().len())
				.sum::<usize>()
				<= budget
		);
		assert_eq!(json!(context.history[0])["result"], "keep");
	}

	#[rstest::rstest]
	fn human_media_takes_the_first_inference_when_selected_files_exceed_the_combined_cap() {
		let image = || crate::provider::ContentPart::Image {
			media_type: "image/png".into(),
			bytes: vec![1],
		};
		let (parts, deferred) = super::choose_inference_media(
			(0..8).map(|_| image()).collect(),
			vec![image()],
			128_000,
			true,
			&media_model(),
		);
		assert!(deferred);
		assert_eq!(parts.len(), 1);
	}

	#[rstest::rstest]
	fn human_media_takes_the_first_inference_when_selected_audio_exceeds_headroom() {
		let audio = || crate::provider::ContentPart::Audio {
			format: "wav".into(),
			bytes: vec![0; 1024 * 1024],
		};
		let (parts, deferred) = super::choose_inference_media(
			vec![audio()],
			vec![audio()],
			128_000,
			true,
			&media_model(),
		);
		assert!(deferred);
		assert_eq!(parts.len(), 1);
	}

	#[rstest::rstest]
	fn text_only_batch_defers_selected_media_when_its_headroom_is_used() {
		let selected = crate::provider::ContentPart::Audio {
			format: "wav".into(),
			bytes: vec![0; 64 * 1024],
		};
		let (parts, deferred) =
			super::choose_inference_media(vec![selected], Vec::new(), 2_048, true, &media_model());
		assert!(deferred);
		assert!(parts.is_empty());
	}

	#[rstest::rstest]
	fn selected_media_waits_when_human_media_needs_another_route() {
		let selected = crate::provider::ContentPart::Image {
			media_type: "image/jpeg".into(),
			bytes: vec![1],
		};
		let human = crate::provider::ContentPart::Image {
			media_type: "image/png".into(),
			bytes: vec![2],
		};
		let (parts, deferred) = super::choose_inference_media(
			vec![selected],
			vec![human],
			128_000,
			true,
			&media_model(),
		);
		assert!(deferred);
		assert!(
			matches!(parts.as_slice(), [crate::provider::ContentPart::Image {media_type, ..}] if media_type == "image/png")
		);
	}

	#[rstest::rstest]
	fn selected_files_must_share_one_current_route() {
		let png = crate::provider::ContentPart::Image {
			media_type: "image/png".into(),
			bytes: b"\x89PNG\r\n\x1a\nfirst".to_vec(),
		};
		let jpeg = crate::provider::ContentPart::Image {
			media_type: "image/jpeg".into(),
			bytes: b"\xff\xd8\xffsecond".to_vec(),
		};
		let model = media_model();
		assert!(super::check_model_media_headroom(128_000, vec![png.clone()], &model).is_ok());
		assert!(super::check_model_media_headroom(128_000, vec![jpeg.clone()], &model).is_ok());
		assert!(matches!(
			super::check_model_media_headroom(128_000, vec![png, jpeg], &model),
			Err(Error::Invalid(message)) if message.contains("no current media route")
		));
	}

	#[rstest::rstest]
	fn encoded_message_text_consumes_media_headroom() {
		let text = "quoted \"text\" and newline\n".repeat(100);
		let reservation = super::encoded_run_message_reservation(&[json!({
			"seq":1,"sender":"human","content":text
		})]);
		assert!(reservation > text.len());
		let image = crate::provider::ContentPart::Image {
			media_type: "image/png".into(),
			bytes: vec![1],
		};
		let headroom = 4_096 + reservation / 2;
		assert!(super::media_request_headroom(headroom, std::slice::from_ref(&image)).is_ok());
		assert!(super::media_request_headroom(headroom - reservation, &[image]).is_err());
	}

	#[rstest::rstest]
	fn selected_media_defers_workspace_mutations_until_the_next_inference() {
		for name in ["file_read", "file_search", "workspace_read", "skill_read"] {
			assert!(super::read_only_after_model_media_selection(name));
		}
		for name in [
			"skill_load",
			"apply_patch",
			"shell",
			"code_interpreter",
			"plugin_0",
		] {
			assert!(!super::read_only_after_model_media_selection(name));
		}
	}

	#[rstest::rstest]
	fn deferred_tool_transitions_keep_selected_media() {
		let first = json!({"file_id":Uuid::new_v4(),"expected_digest":"first"});
		let second = json!({"file_id":Uuid::new_v4(),"expected_digest":"second"});
		let pending = pending(json!({
			"deferred_selected_media":[first],
			"selected_media":[second]
		}));
		assert_eq!(
			json!(super::pending_selected_media(&pending)),
			json!([first, second])
		);
	}

	#[rstest::rstest]
	fn selected_audio_must_fit_the_model_context_before_tool_completion() {
		let mut bytes = vec![0_u8; 8 * 1024 * 1024];
		bytes[..12].copy_from_slice(b"RIFF\0\0\0\0WAVE");
		let part = crate::provider::ContentPart::from_media("audio/wav", bytes).unwrap();
		assert!(matches!(
			super::check_model_media_headroom(128_000, vec![part], &media_model()),
			Err(Error::Invalid(message)) if message.contains("context window")
		));
	}

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
			recovery_cursors: Default::default(),
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
			id,
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
		let full_event = typed_event(
			serde_json::json!({"kind":"tool","call":call,"result":super::skill_read_result(&output, 16000)}),
		);
		let full_growth = crate::context::tool_event_growth(&context, &full_event);
		let minimum_event = typed_event(
			serde_json::json!({"kind":"tool","call":call,"result":super::skill_read_result(&output, 0)}),
		);
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
		let event =
			typed_event(serde_json::json!({"kind":"tool","call":bounded_call,"result":result}));
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
			let event = typed_event(serde_json::json!({
				"kind":"tool",
				"call":bounded_call,
				"result":super::skill_read_result(&output, bytes)
			}));
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
		let event =
			typed_event(serde_json::json!({"kind":"tool","call":bounded_call,"result":result}));
		assert!(request_tokens + crate::context::tool_event_growth(&context, &event) <= allowed);
		let mut old_quota_call = call.clone();
		old_quota_call.arguments["max_chars"] = serde_json::json!(chars + 1);
		let old_quota_result = super::workspace_read_result(&output, 16000, 0, chars + 1);
		let old_quota_event = typed_event(
			serde_json::json!({"kind":"tool","call":old_quota_call,"result":old_quota_result}),
		);
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
		let event =
			typed_event(serde_json::json!({"kind":"tool","call":too_large,"result":result}));
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
			&typed_event(serde_json::json!({
				"kind":"tool",
				"call":{"id":"observe-1","name":"workspace_observe","arguments":{"offset":0,"limit":1}},
				"result":output
			})),
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
		let pending = pending(serde_json::json!({
			"workspace_read_plan": {
				"step": 4,
				"cursor": 0,
				"call":{"id":"read-1","name":"workspace_read","arguments":{}},
				"result": {"content":"previously prepared"}
			}
		}));
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
		assert_eq!(json!(deferred.call)["arguments"]["offset"], 0);
		assert_eq!(
			json!(deferred.call)["arguments"]["id"],
			call.arguments["id"]
		);
	}

	#[rstest::rstest]
	fn referenced_run_message_requires_every_record_chunk() {
		let id = uuid::Uuid::new_v4();
		let event = |offset: usize, content: &str, next: Option<usize>| {
			typed_event(serde_json::json!({
				"kind":"tool",
				"call":{"id":"read-message","name":"workspace_read","arguments":{"kind":"message","id":id}},
				"result":{"kind":"message","id":id,"encoding":"json","offset":offset,"total_chars":6,"content":content,"next_offset":next}
			}))
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
