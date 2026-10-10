//! Native persistence and authority adapters for the application agent executor.
use crate::{
	authorization::execution::Guard,
	federation::{Federation, Home},
	registry::AgentConfig,
	registry::Registry,
	store::Store,
};
use aidash_application::{
	Result,
	ports::{CompactionClassifier, CompactionQuestions, ModelProvider, execution::*},
};
use aidash_domain::{
	media::Selection,
	model::ModelConfig,
	provider::{ContentPart, ModelRequest, ModelResponse, ToolCall},
	registry::Entry,
	semantic::InputRead,
	*,
};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

#[async_trait]
impl ExecutionStore for Store {
	async fn observe_sources(&self, run: &mut Run, token: Uuid) -> Result<()> {
		*run = Store::save_run(self, run, token, "run.sources_observed").await?;
		Ok(())
	}
	async fn save_run(&self, run: &Run, token: Uuid, event: &str) -> Result<()> {
		Store::save_run(self, run, token, event)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn emit(&self, workspace: Option<Uuid>, kind: &str, data: Value) -> Result<Event> {
		Store::emit(self, workspace, kind, data)
			.await
			.map_err(Into::into)
	}
	async fn run_inputs(&self, run: Uuid) -> Result<Vec<aidash_domain::run_input::RunInput>> {
		Store::run_inputs(self, run).await.map_err(Into::into)
	}
	async fn begin_final_completion(&self, run: &Run, token: Uuid) -> Result<bool> {
		Store::begin_final_completion(self, run, token)
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
		Store::human_request(self, run, kind, prompt, key)
			.await
			.map_err(Into::into)
	}
	async fn human_request_by_id(&self, id: Uuid) -> Result<HumanRequest> {
		Store::human_request_by_id(self, id)
			.await
			.map_err(Into::into)
	}
	async fn expire_workbench_approval(&self, id: Uuid) -> Result<HumanRequest> {
		Store::expire_workbench_approval(self, id)
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
		let result =
			Store::invocation_start(self, run, token, key, name, input, replay_safe).await?;
		Ok(InvocationOutcome {
			status: result.status,
			result: result.result,
		})
	}
	async fn invocation_finish(
		&self,
		run: &Run,
		token: Uuid,
		key: &str,
		output: &Value,
	) -> Result<()> {
		Store::invocation_finish(self, run, token, key, output)
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
		Store::reconciliation_request(self, run, token, key, prompt)
			.await
			.map_err(Into::into)
	}
	async fn run_message_has_media(&self, messages: &[Uuid]) -> Result<bool> {
		Store::run_message_has_media(self, messages)
			.await
			.map_err(Into::into)
	}
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
}
struct Classifier(Box<dyn crate::context::jev::JevAsker>);
#[async_trait]
impl CompactionClassifier for Classifier {
	async fn ask(&self, state: &Value, questions: &CompactionQuestions) -> Result<Value> {
		self.0.ask(state, questions).await.map_err(Into::into)
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
	async fn model_media(&self, selections: &[Selection]) -> Result<Vec<ContentPart>> {
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
	async fn suspend(&self) -> Result<()> {
		self.guard.suspend().await.map_err(Into::into)
	}
	async fn resume(&self) -> Result<()> {
		self.guard.resume(self.federation).await.map_err(Into::into)
	}
	fn is_remote(&self) -> bool {
		self.guard.is_remote()
	}
}

pub(crate) struct Environment<'a> {
	federation: &'a Federation,
	authority: Option<Authority<'a>>,
	step_run: Run,
}
impl<'a> Environment<'a> {
	pub(crate) fn new(federation: &'a Federation, guard: Option<&'a Guard>, run: &Run) -> Self {
		Self {
			federation,
			authority: guard.map(|guard| Authority { guard, federation }),
			step_run: run.clone(),
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
		&self.federation.store
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
			projection: config.projection_version,
		})
	}
	fn provider(&self, model: ModelConfig) -> Result<Arc<dyn ModelProvider>> {
		crate::bootstrap::admitted_model_provider(
			&self.federation.store,
			model,
			Some(self.step_run.id),
			String::new(),
			None,
		)
		.map_err(Into::into)
	}
	fn compactor(&self) -> Result<Box<dyn CompactionClassifier>> {
		let classifier: Box<dyn crate::context::jev::JevAsker> =
			if let Some(authority) = &self.authority {
				Box::new(authority.guard.compactor(self.federation))
			} else {
				Box::new(crate::context::jev::JevClient::from_env(
					self.federation.client.clone(),
				)?)
			};
		Ok(Box::new(Classifier(classifier)))
	}
	fn binding_resolver(&self) -> &dyn aidash_application::ports::bindings::BindingResolver {
		self
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
				let value =
					crate::knowledge::load(&self.federation.registry.db, &binding.definition)
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
			let mut lease = crate::semantic::service::Lease::begin(
				&self.federation.store,
				&crate::authorization::identity::Actor::Operator,
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
	async fn prompt_cache_salt(&self, run: &Run) -> Result<String> {
		// The Guard's Tenant belongs to the Run this Environment was built for.
		if run.id != self.step_run.id {
			return Err(aidash_application::Error::Forbidden);
		}
		// A Run without a Guard has no execution grant, no remote admission and
		// an unscoped Workspace (`Guard::begin`), so no Tenant owns it.
		let node = &self.federation.config.node_id;
		let scope = match &self.authority {
			Some(authority) => crate::config::PromptCacheScope::Tenant {
				node,
				tenant: authority.guard.tenant(),
			},
			None => crate::config::PromptCacheScope::Operator(node),
		};
		crate::config::prompt_cache::salt(self.federation.config.prompt_cache.as_ref(), scope)
			.map_err(Into::into)
	}
	async fn semantic_context(
		&self,
		run: &Run,
		task: &Task,
		inputs: &[(InputRead, String)],
		budget: usize,
		entry: &Entry,
		projection: aidash_domain::context::projection::ProjectionVersion,
	) -> Result<aidash_application::ports::execution::SemanticRetrieval> {
		let settings =
			AgentConfig::from_snapshot(run.context.binding_snapshot.as_ref().ok_or_else(
				|| aidash_application::Error::Invalid("Run has no Binding snapshot".into()),
			)?)?;
		if !settings.semantic_memory && !settings.workspace_context {
			// Nothing is retrieved; the absence depends only on the Binding snapshot.
			let dependencies = (!projection.is_legacy())
				.then(|| crate::semantic::services::memory_context::dependencies(None, None))
				.transpose()?;
			return Ok(aidash_application::ports::execution::SemanticRetrieval {
				value: None,
				dependencies,
			});
		}
		if let Some(authority) = &self.authority {
			return authority
				.guard
				.semantic_context(&self.federation.store, task, inputs, budget, projection)
				.await
				.map_err(Into::into);
		}
		let _ = entry;
		let agent = AgentConfig::from_snapshot(run.context.binding_snapshot.as_ref().ok_or_else(
			|| aidash_application::Error::Invalid("Run has no Binding snapshot".into()),
		)?)?;
		let mut lease = crate::semantic::service::Lease::begin(
			&self.federation.store,
			&crate::authorization::identity::Actor::Operator,
		)
		.await?;
		let mut query = format!("{}\n{}", task.title, task.description);
		for (_, text) in inputs {
			query.push('\n');
			query.push_str(text);
		}
		let result = async {
			let workspace_index = if projection.is_legacy() {
				None
			} else {
				crate::semantic::services::memory_context::workspace_index(
					&self.federation.store,
					&mut lease,
					run,
					&agent,
				)
				.await?
			};
			let semantic = crate::semantic::service::context_in(
				&self.federation.store,
				&mut lease,
				run,
				&query,
				crate::semantic::services::memory_context::workspace_budget(budget)?,
				&agent,
			)
			.await?
			.map(serde_json::to_value)
			.transpose()?;
			let reserved =
				serde_json::to_vec(&serde_json::json!({"workspace":semantic,"memory":null}))?.len();
			let memory = crate::semantic::services::memory_context::retrieve(
				&self.federation.store,
				&mut lease,
				run,
				task,
				inputs,
				budget.saturating_sub(reserved),
				&agent,
				projection,
			)
			.await?;
			let dependencies = (!projection.is_legacy())
				.then(|| {
					crate::semantic::services::memory_context::dependencies(
						workspace_index,
						Some(&memory),
					)
				})
				.transpose()?;
			let value = crate::semantic::services::memory_context::complete(
				&mut lease, run, semantic, memory, budget,
			)
			.await?;
			Ok::<_, crate::Error>(aidash_application::ports::execution::SemanticRetrieval {
				value,
				dependencies,
			})
		}
		.await;
		lease.finish(result).await.map_err(Into::into)
	}
	async fn semantic_observation_current(
		&self,
		run: &Run,
		semantic: &Value,
		dependencies: &Value,
	) -> Result<bool> {
		if let Some(authority) = &self.authority {
			return authority
				.guard
				.semantic_observation_current(&self.federation.store, semantic, dependencies)
				.await
				.map_err(Into::into);
		}
		let mut lease = crate::semantic::service::Lease::begin(
			&self.federation.store,
			&crate::authorization::identity::Actor::Operator,
		)
		.await?;
		let result = crate::semantic::services::memory_context::current(
			&self.federation.store,
			&mut lease,
			run,
			semantic,
			dependencies,
		)
		.await;
		lease.finish(result).await.map_err(Into::into)
	}

	async fn run_message_limit(&self, run: &Run) -> Result<usize> {
		self.federation
			.run_message_limit(run)
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

	async fn wait_for_inference_cancellation(&self, run: Uuid) -> Result<()> {
		super::super::services::runtime::wait_for_inference_cancellation(
			&self.federation.store,
			run,
		)
		.await
		.map_err(Into::into)
	}
	async fn operator_human_message_media(
		&self,
		run: &Run,
		messages: &[(i64, Uuid, usize)],
		model: &ModelConfig,
	) -> Result<HumanMediaBatch> {
		crate::authorization::execution::operator_human_message_media(
			&self.federation.store,
			run,
			messages,
			model,
		)
		.await
		.map_err(Into::into)
	}
}

#[async_trait]
impl aidash_application::ports::bindings::BindingResolver for Environment<'_> {
	async fn tools(&self, run: &Run) -> Result<Tools> {
		let providers = Arc::new(super::bindings::Providers {
			federation: self.federation.clone(),
			home: self.native_home(),
		});
		let authority = if let Some(authority) = &self.authority {
			authority.guard.binding_authority()
		} else {
			Arc::new(super::bindings::OperatorAuthority {
				federation: self.federation.clone(),
			}) as Arc<dyn aidash_application::ports::bindings::BindingAuthority>
		};
		aidash_application::registry::bindings::execution::PinnedResolver {
			providers,
			authority,
		}
		.tools(run)
		.await
	}
}
