//! Native persistence and authority adapters for the application agent executor.
use crate::{
	authorization::execution::Guard,
	federation::{Federation, Home},
	registry::AgentConfig,
	registry::Registry,
	store::{RunStore, Store},
};
use aidash_application::ports::execution::completion;
use aidash_application::tenancy::TenancyScope;
use aidash_application::{
	Result,
	ports::{ModelProvider, execution::*},
};
use aidash_domain::{
	media::ModelMedia,
	model::ModelConfig,
	provider::{
		ContentPart, ModelRequest, ModelResponse, ToolCall,
		progress::{InferenceAttemptId, InferenceProgress, ProgressOutcome},
		usage::{UsageDispatch, UsageOutcome},
	},
	registry::Entry,
	semantic::InputRead,
	*,
};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

#[async_trait]
impl ExecutionStore for RunStore {
	async fn research_revoked(&self, run: &Run) -> Result<bool> {
		use aidash_application::ports::research::EvidenceStore;
		super::research::Evidence {
			store: self.store.clone(),
			scope: self.scope.clone(),
		}
		.revoked(run.id)
		.await
	}
	async fn expired_research(
		&self,
		run: &Run,
	) -> Result<aidash_domain::research::expiry::ExpiredResults> {
		let expired: crate::Result<_> = async {
			let mut tx = self
				.store
				.pool
				.begin(&self.scope)
				.await
				.and_then(crate::database::native::Transaction::into_executor)?;
			let expired = super::research::account_terms::expired(tx.as_mut(), run.id).await?;
			tx.commit().await?;
			Ok(expired)
		}
		.await;
		expired.map_err(Into::into)
	}
	async fn unknown_evidence(
		&self,
		run: &Run,
		references: &[aidash_domain::research::citations::EvidenceRef],
	) -> Result<Vec<aidash_domain::research::citations::EvidenceRef>> {
		use aidash_application::ports::research::EvidenceStore;
		let delivered = super::research::Evidence {
			store: self.store.clone(),
			scope: self.scope.clone(),
		}
		.fragments(run.id, references)
		.await?;
		Ok(references
			.iter()
			.filter(|reference| {
				!delivered
					.iter()
					.any(|fragment| fragment.reference == **reference)
			})
			.copied()
			.collect())
	}
	async fn observe_sources(&self, run: &mut Run, token: Uuid) -> Result<()> {
		*run =
			Store::save_run(&self.store, &self.scope, run, token, "run.sources_observed").await?;
		Ok(())
	}
	async fn degrade_sources(&self, run: &mut Run, token: Uuid) -> Result<()> {
		*run = Store::save_run(
			&self.store,
			&self.scope,
			run,
			token,
			"run.semantic_degraded",
		)
		.await?;
		Ok(())
	}
	async fn recover_sources(&self, run: &mut Run, token: Uuid) -> Result<()> {
		*run = Store::save_run(
			&self.store,
			&self.scope,
			run,
			token,
			"run.semantic_recovered",
		)
		.await?;
		Ok(())
	}
	async fn pin_estimate(&self, run: &mut Run, token: Uuid) -> Result<()> {
		*run = Store::save_run(&self.store, &self.scope, run, token, "run.estimate_pinned").await?;
		Ok(())
	}
	async fn pin_plugin_tools(&self, run: &mut Run, token: Uuid) -> Result<()> {
		*run = Store::save_run(
			&self.store,
			&self.scope,
			run,
			token,
			"run.plugin_tools_pinned",
		)
		.await?;
		Ok(())
	}
	async fn calibration_samples(
		&self,
		upstream_model: &str,
		projection_version: u8,
	) -> Result<Vec<aidash_domain::provider::estimate::CalibrationSample>> {
		Store::calibration_samples(&self.store, upstream_model, projection_version)
			.await
			.map_err(Into::into)
	}
	async fn save_run(&self, run: &Run, token: Uuid, event: &str) -> Result<()> {
		Store::save_run(&self.store, &self.scope, run, token, event)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn start_inference(
		&self,
		run: &Run,
		token: Uuid,
		attempt: InferenceAttemptId,
	) -> Result<()> {
		Store::start_inference(&self.store, &self.scope, run.id, token, attempt)
			.await
			.map_err(Into::into)
	}
	async fn append_inference_progress(
		&self,
		run: &Run,
		token: Uuid,
		attempt: InferenceAttemptId,
		batch: &[InferenceProgress],
	) -> Result<()> {
		Store::append_inference_progress(&self.store, &self.scope, run.id, token, attempt, batch)
			.await
			.map_err(Into::into)
	}
	async fn finish_inference(
		&self,
		run: &Run,
		token: Uuid,
		attempt: InferenceAttemptId,
		outcome: ProgressOutcome,
	) -> Result<()> {
		Store::finish_inference(&self.store, &self.scope, run.id, token, attempt, outcome)
			.await
			.map_err(Into::into)
	}
	async fn emit(&self, workspace: Option<Uuid>, kind: &str, data: Value) -> Result<Event> {
		Store::emit(&self.store, &self.scope, workspace, kind, data)
			.await
			.map_err(Into::into)
	}
	async fn run_inputs(&self, run: Uuid) -> Result<Vec<aidash_domain::run_input::RunInput>> {
		Store::run_inputs(&self.store, &self.scope, run)
			.await
			.map_err(Into::into)
	}
	async fn begin_final_completion(&self, run: &Run, token: Uuid) -> Result<bool> {
		Store::begin_final_completion(&self.store, &self.scope, run, token)
			.await
			.map_err(Into::into)
	}
	async fn human_request(
		&self,
		run: &Run,
		kind: &str,
		prompt: &str,
		key: &str,
	) -> Result<HumanRequest> {
		Store::human_request(&self.store, &self.scope, run, kind, prompt, key)
			.await
			.map_err(Into::into)
	}
	async fn human_request_by_id(&self, id: Uuid) -> Result<HumanRequest> {
		Store::human_request_by_id(&self.store, &self.scope, id)
			.await
			.map_err(Into::into)
	}
	async fn invocation_start(
		&self,
		run: &Run,
		token: Uuid,
		key: &str,
		name: &str,
		input: &Value,
		replay_safe: bool,
	) -> Result<InvocationOutcome> {
		let result = Store::invocation_start(
			&self.store,
			&self.scope,
			run,
			token,
			key,
			name,
			input,
			replay_safe,
		)
		.await?;
		Ok(InvocationOutcome {
			status: result.status,
			result: result.result,
		})
	}
	async fn invocation_start_batch(
		&self,
		run: &Run,
		token: Uuid,
		calls: &[BatchInvocation<'_>],
	) -> Result<Vec<InvocationOutcome>> {
		let calls: Vec<_> = calls
			.iter()
			.map(|call| (call.key, call.name, call.input))
			.collect();
		Ok(
			Store::invocation_start_batch(&self.store, &self.scope, run, token, &calls)
				.await?
				.into_iter()
				.map(|invocation| InvocationOutcome {
					status: invocation.status,
					result: invocation.result,
				})
				.collect(),
		)
	}
	async fn invocation_finish(
		&self,
		run: &Run,
		token: Uuid,
		key: &str,
		output: &Value,
	) -> Result<Value> {
		Store::invocation_finish(&self.store, &self.scope, run, token, key, output)
			.await
			.map_err(Into::into)
	}
	async fn reconciliation_request(
		&self,
		run: &mut Run,
		token: Uuid,
		key: &str,
		prompt: &str,
	) -> Result<()> {
		Store::reconciliation_request(&self.store, &self.scope, run, token, key, prompt)
			.await
			.map_err(Into::into)
	}
	async fn record_usage_dispatch(
		&self,
		run: &Run,
		token: Uuid,
		dispatch: &UsageDispatch,
	) -> Result<()> {
		Store::record_usage_dispatch(&self.store, &self.scope, run, token, dispatch)
			.await
			.map_err(Into::into)
	}
	async fn complete_usage_record(
		&self,
		run: &Run,
		attempt: Uuid,
		outcome: &UsageOutcome,
	) -> Result<()> {
		Store::complete_usage_record(&self.store, &self.scope, run, attempt, outcome)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn run_message_has_media(&self, messages: &[Uuid]) -> Result<bool> {
		Store::run_message_has_media(&self.store, &self.scope, messages)
			.await
			.map_err(Into::into)
	}
	async fn context_journal(
		&self,
		run: Uuid,
		from: u64,
		through: u64,
	) -> Result<Vec<aidash_domain::context::HistoryEntry>> {
		Store::context_journal(&self.store, &self.scope, run, from, through)
			.await
			.map_err(Into::into)
	}
	async fn journal_context(&self, run: &Run, token: Uuid) -> Result<()> {
		Store::journal_context(&self.store, &self.scope, run, token)
			.await
			.map_err(Into::into)
	}
	async fn begin_compaction(
		&self,
		run: &Run,
		token: Uuid,
		attempt: &aidash_domain::context::recovery::Attempt,
		call_budget: u32,
	) -> Result<()> {
		Store::begin_compaction(&self.store, &self.scope, run, token, attempt, call_budget)
			.await
			.map_err(Into::into)
	}
	async fn settle_compaction(
		&self,
		run: &Run,
		token: Uuid,
		attempt: Uuid,
		settlement: &aidash_domain::context::recovery::Settlement,
	) -> Result<()> {
		Store::settle_compaction(&self.store, &self.scope, run, token, attempt, settlement)
			.await
			.map_err(Into::into)
	}
	async fn adopt_compaction(
		&self,
		run: &Run,
		token: Uuid,
		attempt: Uuid,
		settlement: &aidash_domain::context::recovery::Settlement,
		staged: Option<&aidash_application::decision::StagedDecision>,
	) -> Result<()> {
		let journal = match staged {
			Some(staged) => Some(staged_journal(&self.store, run, staged)?),
			None => None,
		};
		Store::adopt_compaction(
			&self.store,
			&self.scope,
			run,
			token,
			attempt,
			settlement,
			journal.as_ref().zip(staged),
		)
		.await
		.map_err(Into::into)
	}
}

/// The Decision Journal that records `staged` with the summary adopted for
/// `run`. A Run whose Binding names the Decider rechecks that Binding; a
/// generated or RequiredHome Run keeps the Decider its chain agreed on.
fn staged_journal<'a>(
	store: &'a Store,
	run: &Run,
	staged: &aidash_application::decision::StagedDecision,
) -> Result<crate::apps::execution::repositories::decisions::RunDecisionJournal<'a>> {
	use crate::apps::execution::repositories::decisions::{
		AdmittedAllowance, ChainAllowance, RunDecisionJournal,
	};
	// Recording the decision charges nothing, so no call limit applies here.
	let allowance = aidash_domain::decision::policy::ExecutionAllowance {
		decider: staged.evidence.decider.clone(),
		max_calls_per_run: u64::MAX,
	};
	// Only a Binding that pins the very Decider which staged the decision
	// rechecks it; a chain decision under a different Agent Decider keeps
	// the chain's allowance.
	let admitted = run
		.context
		.binding_snapshot
		.as_deref()
		.map(|snapshot| snapshot.bound_decider(aidash_domain::decision::Hook::Compaction))
		.transpose()?
		.flatten()
		.is_some_and(|(bound, _)| bound.allowance().decider == staged.evidence.decider);
	Ok(if admitted {
		RunDecisionJournal::new(store, run.clone(), allowance, AdmittedAllowance)?
	} else {
		RunDecisionJournal::new(
			store,
			run.clone(),
			allowance.clone(),
			ChainAllowance(allowance),
		)?
	})
}

#[async_trait]
impl ExecutionCatalog for Registry {
	async fn get_for_run(&self, run: &Run, id: &str, version: &str) -> Result<Entry> {
		let snapshot = run.context.binding_snapshot.as_ref().ok_or_else(|| {
			aidash_application::Error::Invalid("Run has no admitted Binding snapshot".into())
		})?;
		snapshot
			.definitions
			.iter()
			.find(|d| {
				d.identity.id == id
					&& d.identity.version == version
					&& d.identity.registry_node == snapshot.agent.registry_node
			})
			.map(|d| d.definition.clone())
			.ok_or_else(|| {
				aidash_application::Error::Invalid(
					"definition is outside the admitted Run closure".into(),
				)
			})
	}
	fn skill_instructions(&self, entry: &Entry) -> Result<String> {
		crate::registry::skill_instructions(entry).map_err(Into::into)
	}
	fn content_digest(&self, content: &str) -> String {
		crate::semantic::service::content_digest(content)
	}
}

struct NativeHome(Home);

#[async_trait]
impl ExecutionHome for NativeHome {
	async fn human_request(
		&self,
		_: &Run,
		kind: &str,
		prompt: &str,
		key: &str,
	) -> Result<HumanRequest> {
		self.0
			.human_request(kind, prompt, key)
			.await
			.map_err(Into::into)
	}
	async fn human_request_by_id(&self, id: Uuid) -> Result<HumanRequest> {
		self.0.human_request_by_id(id).await.map_err(Into::into)
	}
	async fn close_human_request(&self, id: Uuid, response: Value) -> Result<HumanRequest> {
		self.0
			.close_human_request(id, response)
			.await
			.map_err(Into::into)
	}

	async fn task(&self) -> Result<Task> {
		Home::task(&self.0).await.map_err(Into::into)
	}
	async fn claim(&self, task: &Task, agent: &Entry) -> Result<Task> {
		Home::claim(&self.0, task, agent).await.map_err(Into::into)
	}
	async fn transition(&self, next: TaskStatus) -> Result<Task> {
		Home::transition(&self.0, next).await.map_err(Into::into)
	}
	async fn read_record(&self, kind: &str, id: &str) -> Result<Value> {
		Home::read_record(&self.0, kind, id)
			.await
			.map_err(Into::into)
	}
	async fn read_record_chunk(
		&self,
		kind: &str,
		id: &str,
		offset: usize,
		maximum: usize,
	) -> Result<Value> {
		Home::read_record_chunk(&self.0, kind, id, offset, maximum)
			.await
			.map_err(Into::into)
	}
	async fn observation(&self, offset: usize, limit: usize) -> Result<Value> {
		Home::observation(&self.0, offset, limit)
			.await
			.map_err(Into::into)
	}
	async fn observation_fitted(
		&self,
		offset: usize,
		limit: usize,
		fits: &ObservationFit<'_>,
	) -> Result<Option<(usize, Value)>> {
		self.0
			.observation_fitted(offset, limit, |count, value| {
				fits(count, value).map_err(crate::Error::from)
			})
			.await
			.map_err(Into::into)
	}
	async fn child_summary(&self, parent: Uuid) -> Result<ChildTaskSummary> {
		Home::child_summary(&self.0, parent)
			.await
			.map_err(Into::into)
	}
	async fn complete(&self, key: &str, artifact: &ArtifactInput) -> Result<Task> {
		Home::complete(&self.0, key, artifact)
			.await
			.map_err(Into::into)
	}
	async fn report(&self, key: &str, kind: &str, data: Value) -> Result<()> {
		Home::report(&self.0, key, kind, data)
			.await
			.map_err(Into::into)
	}
	async fn response_message(
		&self,
		token: Uuid,
		input_sequence: i64,
		key: &str,
		text: &str,
	) -> Result<()> {
		Home::response_message(&self.0, token, input_sequence, key, text)
			.await
			.map_err(Into::into)
	}
	fn local(&self) -> bool {
		self.0.local()
	}
	fn has_local_authority(&self) -> bool {
		self.0.authority.is_some()
	}
}

struct Reservation(crate::generation::budget::InferenceReservation);
#[async_trait]
impl InferenceReservation for Reservation {
	async fn settle(self: Box<Self>, response: &ModelResponse) -> Result<()> {
		let Self(reservation) = *self;
		reservation.settle(response).await.map_err(Into::into)
	}
	async fn abort(self: Box<Self>) -> Result<()> {
		let Self(reservation) = *self;
		reservation.abort().await.map_err(Into::into)
	}
}
pub(crate) struct Visibility<'a> {
	pub visibility: &'a mut crate::transactions::gate::ReadLease,
	pub store: &'a Store,
}
#[async_trait]
impl ExecutionVisibility for Visibility<'_> {
	async fn suspend(&mut self) -> Result<()> {
		self.visibility.suspend().await.map_err(Into::into)
	}
	async fn resume(&mut self) -> Result<()> {
		self.visibility.resume(self.store).await.map_err(Into::into)
	}
}
struct Authority<'a> {
	guard: &'a Guard,
	federation: &'a Federation,
}

#[async_trait]
impl ExecutionAuthority for Authority<'_> {
	async fn action(&self, action: &str, kind: &str, id: Uuid) -> Result<()> {
		self.guard
			.action(action, kind, id)
			.await
			.map_err(Into::into)
	}
	async fn inference(&self) -> Result<()> {
		self.guard.inference().await.map_err(Into::into)
	}
	async fn tool(
		&self,
		call: &ToolCall,
		contract: &aidash_domain::tool::ToolContract,
	) -> Result<()> {
		self.guard
			.tool_contract(call, contract)
			.await
			.map_err(Into::into)
	}
	async fn human_read(&self, id: Uuid) -> Result<()> {
		self.guard.human_read(id).await.map_err(Into::into)
	}
	async fn tool_call_approval(
		&self,
		run: &Run,
		request: aidash_application::ports::capabilities::approvals::ToolCallRequest,
	) -> Result<aidash_application::ports::capabilities::approvals::ToolCallApproval> {
		// A Run executing for another Node asks that Node through a human request
		// and never reaches this capability approval (ADR 0075).
		let authority = self
			.guard
			.local_authority()
			.ok_or(aidash_application::Error::Forbidden)?;
		authority
			.tool_call_approval(&self.federation.store, run, request)
			.await
			.map_err(Into::into)
	}
	async fn approval_mode(
		&self,
		run: &Run,
	) -> Result<Option<aidash_domain::approval_rules::ApprovalMode>> {
		// A Run executing for another Node is covered by nobody's mode here.
		let Some(authority) = self.guard.local_authority() else {
			return Ok(None);
		};
		authority
			.approval_mode(&self.federation.store, run)
			.await
			.map_err(Into::into)
	}
	async fn auto_judging(&self, run: &Run) -> Result<bool> {
		// A Run executing for another Node is covered by nobody's mode here.
		let Some(authority) = self.guard.local_authority() else {
			return Ok(false);
		};
		authority
			.auto_judging(&self.federation.store, run)
			.await
			.map_err(Into::into)
	}
	async fn answer_deadline(&self, run: &Run) -> Result<Option<i64>> {
		// A Run executing for another Node is covered by nobody's settings here.
		let Some(authority) = self.guard.local_authority() else {
			return Ok(None);
		};
		authority
			.answer_deadline(&self.federation.store, run)
			.await
			.map_err(Into::into)
	}
	async fn approval_judgement(
		&self,
		run: &Run,
		approval: Uuid,
	) -> Result<Option<aidash_application::capabilities::approvals::JudgeRequest>> {
		// A Run executing for another Node is covered by nobody's mode here.
		let Some(authority) = self.guard.local_authority() else {
			return Ok(None);
		};
		authority
			.approval_judgement(&self.federation.store, run, approval)
			.await
			.map_err(Into::into)
	}
	async fn settle_judgement(
		&self,
		run: &Run,
		approval: Uuid,
		settlement: aidash_application::capabilities::approvals::Settlement,
	) -> Result<bool> {
		let authority = self
			.guard
			.local_authority()
			.ok_or(aidash_application::Error::Forbidden)?;
		authority
			.settle_judgement(&self.federation.store, run, approval, settlement)
			.await
			.map_err(Into::into)
	}
	async fn model_media(&self, selections: &[ModelMedia]) -> Result<Vec<ContentPart>> {
		self.guard
			.model_media(&self.federation.store, selections)
			.await
			.map_err(Into::into)
	}
	async fn human_message_media(
		&self,
		messages: &[(i64, Uuid, usize)],
		model: &ModelConfig,
	) -> Result<HumanMediaBatch> {
		self.guard
			.human_message_media(messages, model)
			.await
			.map_err(Into::into)
	}
	async fn reserve_inference(
		&self,
		token: Uuid,
		window: usize,
		output: u32,
		request: &ModelRequest,
	) -> Result<Option<Box<dyn InferenceReservation>>> {
		self.guard
			.reserve_inference(&self.federation.store, token, window, output, request)
			.await
			.map(|value| {
				value.map(|reservation| {
					Box::new(Reservation(reservation)) as Box<dyn InferenceReservation>
				})
			})
			.map_err(Into::into)
	}
	async fn reserve_summary(
		&self,
		token: Uuid,
		summarizer: &aidash_domain::context::summary::SummaryProvider,
		window: usize,
		output: u32,
		request: &ModelRequest,
	) -> Result<Option<Box<dyn InferenceReservation>>> {
		self.guard
			.reserve_summary(self.federation, token, summarizer, window, output, request)
			.await
			.map(|value| {
				value.map(|reservation| {
					Box::new(Reservation(reservation)) as Box<dyn InferenceReservation>
				})
			})
			.map_err(Into::into)
	}
	async fn recheck_summary(
		&self,
		summarizer: &aidash_domain::context::summary::SummaryProvider,
	) -> Result<()> {
		self.guard
			.recheck_summary(self.federation, summarizer)
			.await
			.map_err(Into::into)
	}
	async fn recheck_decider(&self, decider: &aidash_domain::decision::DeciderPin) -> Result<()> {
		crate::apps::execution::repositories::decisions::require_approved(self.guard, decider).await
	}
	async fn suspend(&self) -> Result<()> {
		self.guard.suspend().await.map_err(Into::into)
	}
	/// Hold the authority boundary only for this check so the provider call
	/// stays suspended, whether or not the Run may still infer.
	async fn recheck_inference(&self) -> Result<()> {
		let authorized = match self.guard.resume(self.federation).await {
			Ok(()) => self.guard.inference().await,
			Err(error) => Err(error),
		};
		let suspended = self.guard.suspend().await;
		authorized?;
		suspended.map_err(Into::into)
	}
	async fn resume(&self) -> Result<()> {
		self.guard.resume(self.federation).await.map_err(Into::into)
	}
	fn is_remote(&self) -> bool {
		self.guard.is_remote()
	}
}

impl Environment<'_> {
	/// Completion Gate Checks run only under this node's own authority lease.
	fn worker(&self) -> Option<crate::authorization::execution::WorkerAuthority> {
		self.authority
			.as_ref()
			.and_then(|authority| authority.guard.local_authority())
	}
}

#[async_trait]
impl completion::CompletionChecks for Environment<'_> {
	async fn content(&self, run: &Run) -> Result<Option<completion::MountedContent>> {
		let worker = self.worker().ok_or(aidash_application::Error::Forbidden)?;
		let Some(area) = worker.working_area(&self.federation.store, run).await? else {
			return Ok(None);
		};
		let files: Vec<aidash_domain::capabilities::operations::MountedFile> =
			serde_json::from_value(area.manifest.clone())?;
		Ok(Some(completion::MountedContent {
			digest: aidash_domain::completion::content_digest(&files),
			revision: area.revision,
			state: match area.state.as_str() {
				"active" => completion::ContentState::Ready,
				"running" => completion::ContentState::Busy,
				state => {
					completion::ContentState::Unavailable(format!("the Working Area is {state}"))
				}
			},
		}))
	}
	async fn start(
		&self,
		run: &Run,
		request: &completion::CheckRequest<'_>,
	) -> Result<completion::CheckStart> {
		let worker = self.worker().ok_or(aidash_application::Error::Forbidden)?;
		let store = &self.federation.store;
		// The Check's own timeout, or what an omitted shell argument gets:
		// the shell Binding's ceiling, else the node default. One above the
		// ceiling reaches the Binding's narrowing as declared, which refuses
		// it; an admitted one is never longer than the Run's remaining
		// verification time.
		let ceiling = run
			.context
			.binding_snapshot
			.as_deref()
			.and_then(|snapshot| snapshot.operation(aidash_domain::completion::SHELL).ok())
			.and_then(|binding| binding.narrow.limits.get("timeout_seconds").copied());
		let declared = aidash_domain::completion::check_timeout(
			request.timeout_seconds,
			ceiling,
			store.capabilities.0.operation_seconds,
		);
		let timeout_seconds = if ceiling.is_some_and(|ceiling| declared > ceiling) {
			declared
		} else {
			declared.min(request.remaining_seconds)
		};
		let input = serde_json::json!({
			"idempotency_key": request.key,
			"command": request.command,
			"timeout_seconds": timeout_seconds,
			"expected_revision": request.revision,
		});
		check_started(
			worker
				.check(store, run, input, &request.key.to_string())
				.await,
			timeout_seconds,
		)
	}
	async fn status(&self, run: &Run, operation: Uuid) -> Result<completion::CheckStatus> {
		let worker = self.worker().ok_or(aidash_application::Error::Forbidden)?;
		check_polled(
			worker
				.check_status(&self.federation.store, run, operation)
				.await,
		)
	}
}

/// What starting a Check means to the Completion Gate.
fn check_started(
	result: crate::Result<serde_json::Value>,
	timeout_seconds: u64,
) -> Result<completion::CheckStart> {
	match result {
		Ok(result) => {
			let operation_id = result["operation_id"]
				.as_str()
				.and_then(|id| id.parse().ok())
				.ok_or_else(|| {
					aidash_application::Error::Invalid("Check result has no operation".into())
				})?;
			Ok(completion::CheckStart::Started {
				operation_id,
				timeout_seconds,
			})
		}
		// The content moved after it was read. An idempotency conflict means the
		// same attempt was started at another revision before a process cut;
		// the saved attempt count gives the next start a fresh key.
		Err(crate::Error::Conflict(reason))
			if ["AREA_REVISION_CHANGED", "AREA_BUSY", "IDEMPOTENCY_CONFLICT"]
				.iter()
				.any(|code| reason.starts_with(code)) =>
		{
			Ok(completion::CheckStart::Stale)
		}
		Err(crate::Error::Forbidden | crate::Error::Unauthorized) => Ok(
			completion::CheckStart::Denied("the shell capability is not authorized".into()),
		),
		// The shell Binding's narrowing refused the policy's command or limits:
		// a decision of its authority, not an inability to run.
		Err(crate::Error::Invalid(reason)) if reason.contains("exceeds Binding") => {
			Ok(completion::CheckStart::Denied(reason))
		}
		Err(
			crate::Error::Invalid(reason)
			| crate::Error::Conflict(reason)
			| crate::Error::NotFound(reason),
		) => Ok(completion::CheckStart::Unavailable(reason)),
		Err(error) => Err(error.into()),
	}
}

/// What a polled Check means to the Completion Gate.
fn check_polled(result: crate::Result<serde_json::Value>) -> Result<completion::CheckStatus> {
	match result {
		Ok(result) => Ok(completion::CheckStatus::of(&result)),
		Err(crate::Error::NotFound(reason)) => Ok(completion::CheckStatus::Unavailable(reason)),
		// Revoked authority stops the Run as it does any other step.
		Err(error) => Err(error.into()),
	}
}

#[cfg(test)]
#[path = "../tests/agent_completion_checks.rs"]
mod completion_check_tests;

pub(crate) struct Environment<'a> {
	federation: &'a Federation,
	authority: Option<Authority<'a>>,
	step_run: Run,
	run_store: RunStore,
}
/// How long one Decision Attempt may wait for Jev.
const DECIDER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

impl Environment<'_> {
	/// The Decider's ports for one step: Jev within the version's byte budget,
	/// its Catalog Approval and the Run's Decision Journal.
	async fn decision_ports<'a>(
		&'a self,
		authority: &'a Authority<'a>,
		run: &Run,
		decider: aidash_domain::decision::BoundDecider,
		definition: Entry,
		journal: Box<dyn aidash_application::ports::decision::DecisionJournal + 'a>,
	) -> Result<aidash_application::ports::decision::DecisionPorts<'a>> {
		let provider = aidash_integrations::decision::RouteDecisionProvider::new(
			decider.config.clone(),
			aidash_application::registry::validation::decision_implementation_id(),
			Arc::new(
				crate::apps::identity::repositories::provider_credentials::AdmittedAccess {
					store: self.federation.store.clone(),
				},
			),
			aidash_application::provider_access::Context {
				tenant: authority.guard.tenant().await,
				run: Some(run.id),
				..Default::default()
			},
			Arc::new(aidash_integrations::decision::ByteBudget),
			DECIDER_TIMEOUT,
		)?;
		Ok(aidash_application::ports::decision::DecisionPorts {
			authority: Box::new(
				crate::apps::execution::repositories::decisions::CatalogAuthority {
					guard: authority.guard,
					config: decider.config.clone(),
				},
			),
			journal,
			provider: Box::new(provider),
			decider,
			definition,
		})
	}
}

impl<'a> Environment<'a> {
	pub(crate) fn new(federation: &'a Federation, guard: Option<&'a Guard>, run: &Run) -> Self {
		// A step runs after its Run's authority is admitted. Without it, no Tenant
		// is usable, and the database refuses every statement.
		let scope = guard.map_or(TenancyScope::NoTenant, |guard| guard.tenancy().clone());
		Self {
			federation,
			authority: guard.map(|guard| Authority { guard, federation }),
			step_run: run.clone(),
			run_store: federation.store.for_run(scope),
		}
	}
	fn native_home(&self) -> Home {
		Home::new(self.federation.clone(), self.step_run.clone()).with_authority(
			self.authority
				.as_ref()
				.and_then(|authority| authority.guard.local_authority()),
		)
	}
}

#[async_trait]
impl ExecutionEnvironment for Environment<'_> {
	fn node_id(&self) -> &str {
		&self.federation.config.node_id
	}
	fn store(&self) -> &dyn ExecutionStore {
		&self.run_store
	}
	fn catalog(&self) -> &dyn ExecutionCatalog {
		&self.federation.registry
	}
	fn authority(&self) -> Option<&dyn ExecutionAuthority> {
		self.authority
			.as_ref()
			.map(|authority| authority as &dyn ExecutionAuthority)
	}
	fn home(&self, _run: &Run) -> Box<dyn ExecutionHome> {
		Box::new(NativeHome(self.native_home()))
	}
	fn agent(&self, entry: &Entry) -> Result<ExecutionAgent> {
		let _ = entry;
		let snapshot = self
			.step_run
			.context
			.binding_snapshot
			.as_ref()
			.ok_or_else(|| {
				aidash_application::Error::Invalid("Run has no admitted Binding snapshot".into())
			})?;
		let config = AgentConfig::from_snapshot(snapshot)?;
		Ok(ExecutionAgent {
			model: config.model,
			instructions: config.instructions,
			knowledge_digest: config.knowledge_digest,
			tools: config.tools,
			skills: config.skills,
			max_steps: config.max_steps,
			allow_task_creation: config.allow_task_creation,
			conversation_memory: config.conversation_memory,
			context_policy: config.context_policy,
			tool_parallelism: usize::from(config.tool_parallelism),
			projection_version: config.projection_version.unwrap_or_default(),
			prompt_cache: config.prompt_cache.unwrap_or_default(),
			completion: config.completion,
		})
	}
	fn tool_slots(&self) -> Option<&ToolSlots> {
		Some(&self.federation.store.tool_slots)
	}
	fn provider(&self, model: ModelConfig) -> Result<Arc<dyn ModelProvider>> {
		// Carries this node's Cache Salt Keys for salted Projection Versions.
		crate::bootstrap::admitted_model_provider(
			&self.federation.store,
			model,
			Some(self.step_run.id),
			String::new(),
			None,
		)
		.map_err(Into::into)
	}
	fn binding_resolver(&self) -> &dyn aidash_application::ports::bindings::BindingResolver {
		self
	}
	async fn pin_plugin_tools(
		&self,
		run: &Run,
	) -> Result<Option<aidash_domain::agent_plugin::PluginTools>> {
		let snapshot = run.context.binding_snapshot.as_deref().ok_or_else(|| {
			aidash_application::Error::Invalid("Run has no admitted Binding snapshot".into())
		})?;
		aidash_application::plugin_tools::pin_for_run(
			self.federation.store.plugin_sessions.as_ref(),
			run.id,
			snapshot,
		)
		.await
		.map(Some)
	}
	async fn close_plugin_sessions(&self, run: Uuid) {
		use aidash_application::ports::tools::PluginSessions;
		self.federation.store.plugin_sessions.close(run).await;
	}
	async fn compaction_decider(
		&self,
		run: &Run,
		_token: Uuid,
	) -> Result<Option<aidash_application::ports::decision::DecisionPorts<'_>>> {
		use crate::apps::execution::repositories::decisions::{
			AdmittedAllowance, ChainAllowance, ChainOwners, RunDecisionJournal,
		};
		use aidash_application::generation::compaction::Chain;
		// Only a worker holding the Run's authority may call a Decider.
		let Some(authority) = &self.authority else {
			return Ok(None);
		};
		let store = &self.federation.store;
		match authority.guard.chain_decider(self.federation).await? {
			// A generated chain or RequiredHome execution uses only the Decider
			// its chain agreed on, never the Agent's own Binding (ADR 0042).
			Chain::Generated(None) => Ok(None),
			Chain::Generated(Some(definition)) => {
				let definition = *definition;
				let config: aidash_domain::decision::DeciderConfig =
					serde_json::from_value(definition.config.clone())?;
				let decider = aidash_domain::decision::BoundDecider {
					pin: aidash_domain::decision::DeciderPin {
						identity: aidash_domain::registry::bindings::QualifiedRef {
							registry_node: self.node_id().to_owned(),
							id: definition.id.clone(),
							version: definition.version.clone(),
						},
						definition_digest: aidash_domain::registry::rules::digest(
							&serde_json::to_value(&definition)?,
						),
						configuration_digest: config.digest()?,
						provider_implementation:
							aidash_application::registry::validation::decision_implementation_id(),
					},
					config,
					restrictions: Default::default(),
				};
				let journal = RunDecisionJournal::new(
					store,
					run.clone(),
					decider.allowance(),
					ChainAllowance(decider.allowance()),
				)?
				.with_owners(ChainOwners {
					guard: authority.guard,
					federation: self.federation,
					decider: aidash_domain::registry::EntityRef {
						id: definition.id.clone(),
						version: definition.version.clone(),
					},
					config: decider.config.clone(),
					open: Default::default(),
				});
				self.decision_ports(authority, run, decider, definition, Box::new(journal))
					.await
					.map(Some)
			}
			Chain::Ordinary => {
				let Some(snapshot) = run.context.binding_snapshot.as_deref() else {
					return Ok(None);
				};
				let Some((decider, definition)) =
					snapshot.bound_decider(aidash_domain::decision::Hook::Compaction)?
				else {
					return Ok(None);
				};
				let journal = RunDecisionJournal::new(
					store,
					run.clone(),
					decider.allowance(),
					AdmittedAllowance,
				)?;
				self.decision_ports(authority, run, decider, definition, Box::new(journal))
					.await
					.map(Some)
			}
		}
	}
	async fn approval_decider(
		&self,
		run: &Run,
		_token: Uuid,
	) -> Result<Option<aidash_application::ports::decision::DecisionPorts<'_>>> {
		use crate::apps::execution::repositories::decisions::{ChainAllowance, RunDecisionJournal};
		use aidash_domain::decision::{BoundDecider, DeciderConfig, DeciderPin, Hook};
		// Only a worker holding the Run's authority on its home Node may call a
		// Decider for it. A Run executing for another Node asks that Node.
		let Some(authority) = &self.authority else {
			return Ok(None);
		};
		if authority.guard.local_authority().is_none() {
			return Ok(None);
		}
		let reference = authority.guard.approval_decider_reference().await?;
		// The Decider runs only under its Catalog Approval for this exact version.
		let definition = match authority.guard.decider_entry(&reference).await {
			Ok(entry) => entry,
			Err(crate::Error::Forbidden | crate::Error::NotFound(_)) => return Ok(None),
			Err(error) => return Err(error.into()),
		};
		let Ok(config) = serde_json::from_value::<DeciderConfig>(definition.config.clone()) else {
			return Ok(None);
		};
		if definition.kind != "decider" || config.hook != Hook::Approval {
			return Ok(None);
		}
		let identity = aidash_domain::registry::bindings::QualifiedRef {
			registry_node: self.node_id().to_owned(),
			id: definition.id.clone(),
			version: definition.version.clone(),
		};
		// An Agent's Binding can only make the chosen Decider's thresholds
		// stricter: its narrowing applies whichever Decider the Tenant chose,
		// and the Decider it names is never the judge.
		let restrictions = match &self.step_run.context.binding_snapshot {
			Some(snapshot) => snapshot
				.bound_decider(Hook::Approval)?
				.map(|(bound, _)| bound.restrictions)
				.unwrap_or_default(),
			None => Default::default(),
		};
		if restrictions.validate(&config).is_err() {
			tracing::warn!(
				decider = %identity.id,
				"the Agent's Binding narrows the approval Decider beyond its version; a person is asked"
			);
			return Ok(None);
		}
		let decider = BoundDecider {
			pin: DeciderPin {
				definition_digest: aidash_domain::registry::rules::digest(&serde_json::to_value(
					&definition,
				)?),
				configuration_digest: config.digest()?,
				provider_implementation:
					aidash_application::registry::validation::decision_implementation_id(),
				identity,
			},
			config,
			restrictions,
		};
		// The pin is constant for the step: a withdrawn Catalog Approval is
		// caught by the Decision Authority before every dispatch.
		let journal = RunDecisionJournal::new(
			&self.federation.store,
			run.clone(),
			decider.allowance(),
			ChainAllowance(decider.allowance()),
		)?;
		self.decision_ports(authority, run, decider, definition, Box::new(journal))
			.await
			.map(Some)
	}
	fn completion_checks(&self) -> Option<&dyn completion::CompletionChecks> {
		self.worker()
			.is_some()
			.then_some(self as &dyn completion::CompletionChecks)
	}

	async fn documents(&self, _entry: &Entry) -> Result<Value> {
		let snapshot = self
			.step_run
			.context
			.binding_snapshot
			.as_ref()
			.ok_or_else(|| {
				aidash_application::Error::Invalid("Run has no admitted Binding snapshot".into())
			})?;
		let mut documents = vec![];
		for binding in &snapshot.bindings {
			if binding.excluded_reason.is_some()
				|| binding.definition.kind != "source"
				|| binding.definition.config.get("schema_version").is_none()
			{
				continue;
			}
			let context: aidash_domain::registry::bindings::sources::NativeContext =
				serde_json::from_value(binding.definition.config.clone())?;
			if matches!(
				context.source,
				aidash_domain::registry::bindings::sources::NativeSource::PrivateReferences { .. }
			) {
				let tenant = self.federation.store.run_tenant(self.step_run.id).await?;
				let value =
					crate::knowledge::load(&self.federation.registry, &tenant, &binding.definition)
						.await?;
				documents.extend(
					value
						.as_array()
						.ok_or_else(|| {
							aidash_application::Error::Invalid(
								"private source is not a document list".into(),
							)
						})?
						.iter()
						.cloned(),
				);
			}
		}
		Ok(serde_json::json!(documents))
	}

	async fn recheck_source_observation(&self, run: &Run, content: &Value) -> Result<()> {
		if let Some(authority) = &self.authority {
			authority
				.guard
				.recheck_source_observation(&self.federation.store, content)
				.await?;
		} else if let Some(semantic) = content.get("semantic_memory").filter(|v| !v.is_null()) {
			let mut lease = crate::semantic::service::Lease::within(
				&self.federation.store,
				&self.run_store.scope,
			)
			.await?;
			let result = crate::semantic::services::memory_context::recheck(
				&self.federation.store,
				&mut lease,
				run,
				semantic,
			)
			.await;
			lease.finish(result).await?;
		}
		let _ = run;
		Ok(())
	}

	async fn skill_context(&self, run: &Run) -> Result<String> {
		let settings =
			AgentConfig::from_snapshot(run.context.binding_snapshot.as_ref().ok_or_else(
				|| aidash_application::Error::Invalid("Run has no Binding snapshot".into()),
			)?)?;
		// Support-tool availability does not opt in to mounted Skill Sources.
		if !settings.core_capabilities.skills {
			return Ok(String::new());
		}
		if let Some(authority) = self
			.authority
			.as_ref()
			.and_then(|authority| authority.guard.local_authority())
		{
			authority
				.skill_context(&self.federation.store, run)
				.await
				.map_err(Into::into)
		} else {
			Ok(String::new())
		}
	}
	async fn direct_skills(&self, run: &Run) -> Result<Vec<aidash_domain::exposure::DirectSkill>> {
		match self
			.authority
			.as_ref()
			.and_then(|authority| authority.guard.local_authority())
		{
			Some(authority) => authority
				.direct_skills(&self.federation.store, run)
				.await
				.map_err(Into::into),
			None => Ok(vec![]),
		}
	}
	async fn direct_skill_body(&self, run: &Run, skill_id: Uuid, digest: &str) -> Result<String> {
		self.authority
			.as_ref()
			.and_then(|authority| authority.guard.local_authority())
			.ok_or_else(|| aidash_application::Error::NotFound("skill unavailable".into()))?
			.direct_skill_body(&self.federation.store, run, skill_id, digest)
			.await
			.map_err(Into::into)
	}
	async fn skill_revision(&self, run: &Run) -> Result<Option<i64>> {
		super::skills::revision(&self.federation.store, run.id).await
	}
	async fn retrieval_scope(
		&self,
		run: &Run,
	) -> Result<aidash_domain::context::sources::RetrievalScope> {
		if let Some(authority) = &self.authority {
			return authority
				.guard
				.retrieval_scope(&self.federation.store)
				.await
				.map_err(Into::into);
		}
		// Operator Runs carry no subject policy snapshot; their Tenant is the
		// one whose provider cache this Run already shares.
		let tenant = self.cache_scope(run).await?.tenant;
		let revisions = crate::semantic::services::memory_context::source_revisions(
			&self.federation.store,
			run,
		)
		.await?;
		Ok(aidash_domain::context::sources::RetrievalScope {
			tenant,
			subject: "deployment_administrator".into(),
			authorization_revision: None,
			index_revision: revisions.index,
			participant_revision: revisions.participant,
			corpus_digest: Some(revisions.corpus),
		})
	}
	async fn cache_scope(&self, run: &Run) -> Result<aidash_domain::projection::CacheScope> {
		let keys = self.federation.store.cache_salt.as_ref().ok_or_else(|| {
			aidash_application::Error::Invalid(
				"this node has no Cache Salt Key for salted Projection Versions".into(),
			)
		})?;
		// The executing node's local Tenant: the worker's Access for scoped and
		// remote Runs (the receiver's mapped Tenant), otherwise the Tenant that
		// owns the local workspace.
		let tenant = match &self.authority {
			Some(authority) => authority.guard.tenant().await,
			None => {
				self.federation
					.store
					.workspace_tenant(run.workspace_id)
					.await?
			}
		};
		Ok(aidash_domain::projection::CacheScope {
			tenant,
			key_version: keys.current(),
		})
	}
	async fn semantic_context(
		&self,
		run: &Run,
		task: &Task,
		inputs: &[(InputRead, String)],
		budget: usize,
		entry: &Entry,
		key: Option<&aidash_domain::context::sources::RetrievalKey>,
		degrade_on_outage: bool,
	) -> SemanticContextRead {
		let mut workspace_retrieved = false;
		let result: Result<Option<Value>> = async {
			let settings =
				AgentConfig::from_snapshot(run.context.binding_snapshot.as_ref().ok_or_else(
					|| aidash_application::Error::Invalid("Run has no Binding snapshot".into()),
				)?)?;
			if !settings.semantic_memory && !settings.workspace_context {
				return Ok(None);
			}
			if let Some(authority) = &self.authority {
				let read = authority
					.guard
					.semantic_context(
						&self.federation.store,
						task,
						inputs,
						budget,
						key,
						degrade_on_outage,
					)
					.await;
				workspace_retrieved = read.workspace_retrieved;
				return read.result;
			}
			let _ = entry;
			let agent =
				AgentConfig::from_snapshot(run.context.binding_snapshot.as_ref().ok_or_else(
					|| aidash_application::Error::Invalid("Run has no Binding snapshot".into()),
				)?)?;
			let mut lease = crate::semantic::service::Lease::within(
				&self.federation.store,
				&self.run_store.scope,
			)
			.await?;
			let mut query = format!("{}\n{}", task.title, task.description);
			for (_, text) in inputs {
				query.push('\n');
				query.push_str(text);
			}
			let semantic = crate::semantic::service::context_in(
				&self.federation.store,
				&mut lease,
				run,
				&query,
				crate::semantic::services::memory_context::workspace_budget(budget)?,
				&agent,
			)
			.await;
			let degrade = degrade_on_outage && agent.workspace_degrades;
			let result = async {
				let semantic = match semantic? {
					// Optional workspace context may degrade; memory still follows
					// and still fails closed (ADR 0059).
					aidash_application::semantic::retrieval::WorkspaceContext::Unavailable(
						cause,
					) if degrade => Some(aidash_domain::semantic::degraded::marker(cause)),
					workspace => workspace
						.required()?
						.map(serde_json::to_value)
						.transpose()?,
				};
				workspace_retrieved = semantic.as_ref().is_some_and(|value| {
					aidash_domain::semantic::degraded::degraded(value).is_none()
				});
				let reserved =
					serde_json::to_vec(&serde_json::json!({"workspace":semantic,"memory":null}))?
						.len();
				let memory = crate::semantic::services::memory_context::retrieve(
					&self.federation.store,
					&mut lease,
					run,
					task,
					inputs,
					budget.saturating_sub(reserved),
					&agent,
					key,
				)
				.await?;
				crate::semantic::services::memory_context::complete(
					&mut lease, run, semantic, memory, budget,
				)
				.await
			}
			.await;
			lease.finish(result).await.map_err(Into::into)
		}
		.await;
		SemanticContextRead {
			workspace_retrieved,
			result,
		}
	}

	async fn run_message_limit(&self, run: &Run) -> Result<usize> {
		self.federation
			.run_message_limit(run)
			.await
			.map_err(Into::into)
	}
	async fn url_intents(
		&self,
		run: &Run,
	) -> Result<Vec<aidash_domain::research::intent::UrlIntent>> {
		self.federation
			.store
			.url_intents(&self.run_store.scope, run.id)
			.await
			.map_err(Into::into)
	}
	async fn run_request_headroom(&self, run: &Run) -> Result<usize> {
		self.federation
			.run_request_headroom(run)
			.await
			.map_err(Into::into)
	}
	async fn deliver_run_messages(&self, run: &Run) -> Result<()> {
		self.federation
			.deliver_run_messages(run)
			.await
			.map_err(Into::into)
	}
	async fn reconcile_run_messages(&self, run: &Run) -> Result<()> {
		self.federation
			.reconcile_run_messages(run)
			.await
			.map_err(Into::into)
	}
	async fn require_terminal_safe_delivery(&self, run: &Run) -> Result<()> {
		self.federation
			.require_terminal_safe_delivery(run)
			.await
			.map_err(Into::into)
	}
	async fn transition_terminal_run_messages(&self, run: &Run, status: TaskStatus) -> Result<()> {
		self.federation
			.transition_terminal_run_messages(run, status)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}

	async fn wait_for_inference_interruption(
		&self,
		run: Uuid,
		included_input_seq: i64,
	) -> Result<InferenceInterruption> {
		super::super::services::runtime::wait_for_inference_interruption(
			&self.federation.store,
			&self.run_store.scope,
			run,
			included_input_seq,
		)
		.await
		.map_err(Into::into)
	}
	async fn summary_dependencies_current(
		&self,
		run: &Run,
		dependencies: &aidash_domain::context::summary::SummaryDependencies,
	) -> Result<bool> {
		if dependencies.message_ids.is_empty() {
			return Ok(true);
		}
		if let Some(authority) = &self.authority {
			if run.id != self.step_run.id {
				return Err(aidash_application::Error::Forbidden);
			}
			return authority
				.guard
				.messages_readable(&dependencies.message_ids)
				.await
				.map_err(Into::into);
		}
		// Trusted local execution has no worker policy: a dependency is current
		// while the message still exists in the Run's Workspace.
		for id in &dependencies.message_ids {
			match self
				.federation
				.store
				.workspace_record(&self.run_store.scope, run.workspace_id, "message", *id)
				.await
			{
				Ok(_) => {}
				Err(crate::Error::NotFound(_) | crate::Error::Forbidden) => return Ok(false),
				Err(error) => return Err(error.into()),
			}
		}
		Ok(true)
	}
}

#[async_trait]
impl aidash_application::ports::bindings::BindingResolver for Environment<'_> {
	async fn tools(&self, run: &Run) -> Result<Tools> {
		let providers = Arc::new(super::bindings::Providers {
			federation: self.federation.clone(),
			home: self.native_home(),
			scope: self.run_store.scope.clone(),
		});
		let Some(authority) = &self.authority else {
			return Err(aidash_application::Error::Forbidden);
		};
		let authority = authority.guard.binding_authority();
		aidash_application::registry::bindings::execution::PinnedResolver {
			providers,
			authority,
			sessions: Some(self.federation.store.plugin_sessions.clone()),
		}
		.tools(run)
		.await
	}
}
