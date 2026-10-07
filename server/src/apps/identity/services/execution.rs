use super::{access::Access, catalog, identity::SubjectIdentity};
use crate::apps::identity::repositories::execution::Grant;
use crate::{
	Error, Result,
	apps::execution::serializers::runs::RunDetails,
	domain::*,
	federation::{Delegation, Discovery, Federation},
	provider::ToolCall,
	registry::{AgentConfig, EntityRef, Search},
	store::Store,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

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
	let Some(grant) = grant(store, run).await? else {
		return Ok(None);
	};
	let mut access = Access::begin(store, &grant.identity()).await?;
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
	aidash_application::authorization::execution::refresh_worker(
		&mut crate::bootstrap::execution_grant_scope(access),
		&store.node_id,
		&run.metadata(),
	)
	.await
	.map_err(Into::into)
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
	if node != f.config.node_id {
		return super::remote::execution::delegate(f, identity, task, node, agent).await;
	}
	let mut access = Access::begin(&f.store, identity).await?;
	let result = delegate_in(f, &mut access, task, agent).await;
	let result = access.finish(result).await?;
	f.notify.notify_waiters();
	Ok(result)
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
	access: Arc<Mutex<Access>>,
}

impl WorkerAuthority {
	pub async fn core_tool(
		&self,
		store: &Store,
		run: &Run,
		name: &str,
		input: Value,
		key: &str,
	) -> Result<Value> {
		let outer = self.access.lock().await;
		let mut access = Access::under_lease(&outer).await?;
		let result =
			crate::capabilities::service::invoke(store, &mut access, run, name, input, key).await;
		match access.finish(result).await {
			Ok(result) => Ok(json!(result)),
			Err(
				error @ (Error::Invalid(_)
				| Error::Conflict(_)
				| Error::NotFound(_)
				| Error::Forbidden),
			) => {
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
			Err(error) => Err(error),
		}
	}

	pub async fn memory_mutate(
		&self,
		store: &Store,
		run: &Run,
		key: &str,
		changes: &[aidash_domain::memory::Change],
	) -> Result<Vec<aidash_domain::memory::Unit>> {
		let mut outer = self.access.lock().await;
		if outer.tx.is_active() {
			outer.suspend().await?;
		}
		let mut access = access_for_run(store, &run.metadata(), true)
			.await?
			.ok_or(Error::Forbidden)?;
		if !access.run_visible(run.metadata()).await? {
			return Err(Error::RemoteSemantic(
				crate::semantic::remote::Failure::Invalidated,
			));
		}
		let mut lease = crate::semantic::service::Lease::Scoped(Box::new(access));
		let result =
			crate::semantic::native_memory::run_mutate(store, &mut lease, run, key, changes).await;
		let result = lease.finish(result).await;
		refresh_access_for_run(&mut outer, store, run).await?;
		if !outer.run_visible(run.metadata()).await? {
			return Err(Error::RemoteSemantic(
				crate::semantic::remote::Failure::Invalidated,
			));
		}
		result
	}
	pub async fn memory_recall(
		&self,
		store: &Store,
		run: &Run,
		key: &str,
		query: aidash_domain::memory::RecallQuery,
		reflect: bool,
	) -> Result<crate::semantic::native_memory::Outcome> {
		let mut outer = self.access.lock().await;
		let mut lease = crate::semantic::service::Lease::Inherited(&mut outer);
		crate::semantic::native_memory::run_recall(store, &mut lease, run, key, query, reflect)
			.await
	}

	pub async fn assign(
		&self,
		f: &Federation,
		run: &Run,
		task: Uuid,
		policy: &str,
		reason: &str,
	) -> Result<crate::generation::Assignment> {
		let mut lease = self.access.lock().await;
		// Read potential references under the outer lease before opening the mutation transaction.
		catalog::list_in(&mut lease, &Search::default()).await?;
		let mut access = Access::under_lease(&lease).await?;
		let result = aidash_application::authorization::worker_tasks::assign(
			&mut crate::bootstrap::worker_task_scope(f, &mut access),
			&run.metadata(),
			task,
			policy,
			reason,
		)
		.await
		.map_err(Into::into);
		access.finish(result).await
	}

	pub async fn create_task(
		&self,
		f: &Federation,
		run: &Run,
		key: &str,
		input: &NewTask,
	) -> Result<Task> {
		let lease = self.access.lock().await;
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

	pub async fn snapshot(&self, workspace: Uuid) -> Result<WorkspaceSnapshot> {
		self.access.lock().await.workspace_snapshot(workspace).await
	}

	pub async fn workspace_observation(
		&self,
		workspace: Uuid,
		offset: usize,
		limit: usize,
	) -> Result<Value> {
		self.access
			.lock()
			.await
			.workspace_observation(workspace, offset, limit)
			.await
	}

	pub async fn workspace_observation_fitted<F>(
		&self,
		workspace: Uuid,
		offset: usize,
		limit: usize,
		fits: F,
	) -> Result<Option<(usize, Value)>>
	where
		F: FnMut(usize, &Value) -> Result<bool>,
	{
		self.access
			.lock()
			.await
			.workspace_observation_fitted(workspace, offset, limit, fits)
			.await
	}

	pub async fn skill_context(&self, store: &Store, run: &Run) -> Result<String> {
		let mut access = self.access.lock().await;
		crate::capabilities::skills::context(store, &mut access, run).await
	}
	pub async fn workspace_record(&self, workspace: Uuid, kind: &str, id: Uuid) -> Result<Value> {
		self.access
			.lock()
			.await
			.workspace_record(workspace, kind, id)
			.await
	}

	pub async fn workspace_child_summary(
		&self,
		workspace: Uuid,
		parent: Uuid,
	) -> Result<ChildTaskSummary> {
		self.access
			.lock()
			.await
			.workspace_children(workspace, parent)
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
		let lease = self.access.lock().await;
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
		let mut access = self.access.lock().await;
		super::peer::discovery::discover_in(f, &mut access, search).await
	}
}

pub(crate) async fn operator_human_message_media(
	store: &Store,
	run: &Run,
	messages: &[(i64, Uuid, usize)],
	model: &crate::registry::ModelConfig,
) -> Result<HumanMediaBatch> {
	aidash_application::execution::media::operator(
		&crate::bootstrap::operator_media_repository(store),
		&run.metadata(),
		messages,
		model,
	)
	.await
	.map_err(Into::into)
}

pub(crate) use aidash_application::ports::execution::HumanMediaBatch;

pub(crate) struct Guard {
	remote: Option<Federation>,
	access: Arc<Mutex<Access>>,
	run: Run,
	agent: AgentConfig,
}

pub(in crate::apps::identity) async fn authorize_guard(
	f: &Federation,
	run: &RunMetadata,
	access: &mut Access,
	read_context: bool,
) -> Result<AgentConfig> {
	aidash_application::authorization::execution::guard::authorize(
		&mut crate::bootstrap::run_guard_scope(f, access),
		run,
		read_context,
	)
	.await
	.map_err(Into::into)
}

impl Guard {
	pub async fn begin(f: &Federation, run: &Run) -> Result<Option<Self>> {
		let entry = aidash_application::authorization::worker_entry::execution(
			&crate::bootstrap::worker_entry_repository(f),
			run,
		)
		.await?;
		Ok(entry.map(|entry| Self {
			remote: entry.remote.then(|| f.clone()),
			access: Arc::new(Mutex::new(entry.scope)),
			run: run.clone(),
			agent: entry.agent,
		}))
	}
	pub async fn suspend(&self) -> Result<()> {
		let mut access = self.access.lock().await;
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
			),
		)
		.await
		.map_err(Into::into)
	}

	pub fn local_authority(&self) -> Option<WorkerAuthority> {
		self.remote.is_none().then(|| self.authority())
	}
	pub fn is_remote(&self) -> bool {
		self.remote.is_some()
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
		}
	}
	pub async fn semantic_context(
		&self,
		store: &Store,
		task: &Task,
		inputs: &[(crate::semantic::remote::InputRead, String)],
		budget: usize,
	) -> Result<Option<Value>> {
		let workspace_budget = if self.remote.is_some() {
			budget
		} else {
			crate::semantic::services::memory_context::workspace_budget(budget)?
		};
		let semantic = aidash_application::execution::semantic_context::retrieve(
			&crate::bootstrap::run_semantic_repository(
				store,
				self.remote.as_ref(),
				&self.access,
				&self.run,
				&self.agent,
			),
			task,
			inputs,
			workspace_budget,
		)
		.await
		.map_err(Error::from)?;
		if self.remote.is_some() {
			return Ok(semantic);
		}
		let reserved =
			serde_json::to_vec(&serde_json::json!({"workspace":semantic,"memory":null}))?.len();
		let available = budget.saturating_sub(reserved);
		let mut access = self.access.lock().await;
		let memory = crate::semantic::services::memory_context::retrieve(
			store,
			&mut crate::semantic::service::Lease::Inherited(&mut access),
			&self.run,
			task,
			inputs,
			available,
			&self.agent,
		)
		.await?;
		crate::semantic::services::memory_context::combine(semantic, memory, budget)
	}

	pub async fn human_read(&self, id: Uuid) -> Result<()> {
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

	pub fn compactor(&self, f: &Federation) -> crate::generation::compaction::ApprovedCompactor {
		crate::generation::compaction::ApprovedCompactor {
			access: self.access.clone(),
			store: f.store.clone(),
			run: self.run.clone(),
			client: f.client.clone(),
			remote: self.remote.clone(),
		}
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

	pub async fn inference(&self) -> Result<()> {
		self.refresh_remote().await?;
		let mut access = self.access.lock().await;
		self.authorize_inference_with(&mut access).await
	}

	/// Resolve an explicit one-inference file selection under the same execution
	/// authority as the provider request. Only immutable references are durable.
	pub async fn model_media(
		&self,
		store: &Store,
		selections: &[crate::capabilities::sharing::Selection],
	) -> Result<Vec<crate::provider::ContentPart>> {
		aidash_application::execution::media::selected(
			&mut crate::bootstrap::selected_media(store, &self.run, self.access.clone()),
			selections,
		)
		.await
		.map_err(Into::into)
	}

	pub async fn human_message_media(
		&self,
		messages: &[(i64, Uuid, usize)],
		model: &crate::registry::ModelConfig,
	) -> Result<HumanMediaBatch> {
		let mut access = self.access.lock().await;
		aidash_application::execution::media::authorized(
			&mut crate::bootstrap::scoped_media(&mut access),
			self.run.workspace_id,
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

	pub async fn filter_core_tools(
		&self,
		tools: &mut std::collections::BTreeMap<String, Arc<dyn crate::tool::Tool>>,
	) -> Result<()> {
		aidash_application::authorization::tools::filter(
			&crate::bootstrap::agent_tool_repository(self.remote.as_ref(), &self.access, &self.run),
			&self.agent,
			tools,
			|tool| tool.contract(),
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

pub async fn control(
	f: &Federation,
	identity: &SubjectIdentity,
	id: Uuid,
	action: RunControlAction,
) -> Result<RunInspection> {
	aidash_application::authorization::runs::control(
		&crate::bootstrap::run_control_repository(f, identity),
		id,
		action,
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
		run: details.run,
		invocations: details.invocations.into_iter().map(Into::into).collect(),
		memory: details.memory,
		media_input_routes: details.media_input_routes,
	})
}

pub async fn discover(
	f: &Federation,
	identity: &SubjectIdentity,
	search: &Search,
) -> Result<Discovery> {
	super::peer::discovery::discover(f, identity, search).await
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
	access: Arc<Mutex<Access>>,
	remote: bool,
}

impl DeliveryGuard {
	pub async fn begin(f: &Federation, run: &RunMetadata) -> Result<Option<Self>> {
		let entry = aidash_application::authorization::worker_entry::delivery(
			&crate::bootstrap::worker_entry_repository(f),
			run,
		)
		.await?;
		Ok(entry.map(|entry| Self {
			access: Arc::new(Mutex::new(entry.scope)),
			remote: entry.remote,
		}))
	}
}

impl DeliveryGuard {
	pub fn local_authority(&self) -> Option<WorkerAuthority> {
		(!self.remote).then(|| WorkerAuthority {
			access: self.access.clone(),
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
