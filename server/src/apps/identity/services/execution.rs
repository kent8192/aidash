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
use tokio::sync::RwLock;
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
		store.deployment.as_str(),
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
	if node != f.config.deployment {
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
	access: Arc<RwLock<Access>>,
}

impl WorkerAuthority {
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
		let outer = self.access.read().await;
		let mut access = Access::under_lease(&outer).await?;
		access.shared_area = shared_read;
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

	/// Start a Completion Gate Check (#112) under the worker's current
	/// authority lease, exactly as the model's own shell calls are admitted.
	pub async fn check(&self, store: &Store, run: &Run, input: Value, key: &str) -> Result<Value> {
		let outer = self.access.read().await;
		let mut access = Access::under_lease(&outer).await?;
		let result =
			crate::capabilities::service::verify(store, &mut access, run, input, key).await;
		access.finish(result).await
	}

	/// The current result of a Completion Gate Check this Run started.
	pub async fn check_status(&self, store: &Store, run: &Run, operation: Uuid) -> Result<Value> {
		let outer = self.access.read().await;
		let mut access = Access::under_lease(&outer).await?;
		let result =
			crate::capabilities::service::verification(store, &mut access, run, operation).await;
		access.finish(result).await
	}

	/// The Run's Working Area as the Completion Gate reads it; `None` when the
	/// Run has none.
	pub async fn working_area(
		&self,
		store: &Store,
		run: &Run,
	) -> Result<Option<aidash_domain::capabilities::sessions::Area>> {
		let outer = self.access.read().await;
		let mut access = Access::under_lease(&outer).await?;
		let result = crate::capabilities::service::working_area(store, &mut access, run).await;
		access.finish(result).await
	}

	/// Web disclosure approval under the worker's current authority lease. It
	/// shares the capability approval records, decision API and Run waiting.
	pub async fn web_disclosure(
		&self,
		store: &Store,
		run: &Run,
		request: &aidash_application::capabilities::approvals::DisclosureRequest,
	) -> Result<aidash_application::ports::research::DisclosureState> {
		let outer = self.access.write().await;
		let mut access = Access::under_lease(&outer).await?;
		// Under the Run's research lock, so a revocation that commits after
		// the caller's own check still closes, or prevents, this approval.
		let result = match crate::apps::execution::repositories::research::revoked_under_lock(
			access.tx.as_mut(),
			run.id,
		)
		.await
		{
			Ok(true) => Ok(aidash_application::ports::research::DisclosureState::Revoked),
			Ok(false) => {
				async {
					// A core Run's approval belongs to its Working Area, so its Thread
					// lists it; other Runs have none.
					let area = crate::apps::execution::repositories::sessions::run_area(
						&mut access,
						run.id,
					)
					.await?
					.flatten()
					.map(|area| area.id);
					let request = aidash_application::capabilities::approvals::DisclosureRequest {
						area,
						..request.clone()
					};
					let mut scope =
						crate::bootstrap::approval_scope(Some(store), &mut access, None);
					aidash_application::capabilities::approvals::disclose(
						&mut scope,
						&run.metadata(),
						&request,
					)
					.await
					.map_err(Error::from)
				}
				.await
			}
			Err(error) => Err(error),
		};
		access.finish(result).await
	}

	/// Admit one exact call of a Tenant's HTTP or MCP tool, or open the Tool call
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
			// A core Run's approval belongs to its Working Area, so its Thread
			// lists it; other Runs have none.
			let area =
				crate::apps::execution::repositories::sessions::run_area(&mut access, run.id)
					.await?
					.flatten()
					.map(|area| area.id);
			let request =
				aidash_application::capabilities::approvals::ToolCallRequest { area, ..request };
			let mut scope = crate::bootstrap::approval_scope(Some(store), &mut access, None);
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

	/// Consume a one-time Web disclosure approval of `run` after its attempt
	/// is reserved.
	pub async fn consume_web_disclosure(
		&self,
		store: &Store,
		run: &Run,
		approval: Uuid,
	) -> Result<bool> {
		let outer = self.access.write().await;
		let mut access = Access::under_lease(&outer).await?;
		let result = {
			let mut scope = crate::bootstrap::approval_scope(Some(store), &mut access, None);
			aidash_application::capabilities::approvals::consume_disclosure(
				&mut scope,
				&run.metadata(),
				approval,
			)
			.await
			.map_err(Error::from)
		};
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
		let mut outer = self.access.write().await;
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
			crate::semantic::native_memory::run_recall(store, &mut lease, run, key, query, reflect)
				.await;
		let result = lease.finish(result).await;
		refresh_access_for_run(&mut outer, store, run).await?;
		if !outer.run_visible(run.metadata()).await? {
			return Err(Error::RemoteSemantic(
				crate::semantic::remote::Failure::Invalidated,
			));
		}
		result
	}

	pub async fn assign(
		&self,
		f: &Federation,
		run: &Run,
		task: Uuid,
		policy: &str,
		reason: &str,
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

	pub async fn snapshot(&self, workspace: Uuid) -> Result<WorkspaceSnapshot> {
		self.access
			.write()
			.await
			.workspace_snapshot(workspace)
			.await
	}

	pub async fn workspace_observation(
		&self,
		workspace: Uuid,
		offset: usize,
		limit: usize,
	) -> Result<Value> {
		self.access
			.write()
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
			.write()
			.await
			.workspace_observation_fitted(workspace, offset, limit, fits)
			.await
	}

	pub async fn skill_context(&self, store: &Store, run: &Run) -> Result<String> {
		let mut access = self.access.write().await;
		crate::capabilities::skills::context(store, &mut access, run).await
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
	) -> Result<String> {
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
	pub async fn workspace_record(&self, workspace: Uuid, kind: &str, id: Uuid) -> Result<Value> {
		self.access
			.write()
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
			.write()
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
		let lease = self.access.write().await;
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
		super::peer::discovery::discover_in(f, &mut access, search).await
	}
}

pub(crate) use aidash_application::ports::execution::HumanMediaBatch;

pub(crate) struct Guard {
	remote: Option<Federation>,
	access: Arc<RwLock<Access>>,
	run: Run,
	agent: AgentConfig,
	/// The Run's Tenant, learned from its authority when the step began.
	tenancy: aidash_application::tenancy::TenancyScope,
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
	/// The scope of every transaction this Run's step opens.
	pub(crate) fn tenancy(&self) -> &aidash_application::tenancy::TenancyScope {
		&self.tenancy
	}
	/// The Decider's exact definition, once the Run's subject may invoke it here:
	/// its Catalog Approval on this Node (ADR 0042).
	pub(crate) async fn decider_entry(
		&self,
		reference: &EntityRef,
	) -> Result<crate::registry::Entry> {
		let mut access = self.access.write().await;
		catalog::entry(&mut access, reference, aidash_domain::decision::INVOKE).await
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
			// from a Home whose protocol predates it or that grants no semantic access.
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
			// Required-Home semantic context never degrades (ADR 0059).
			degrade_on_outage && self.remote.is_none() && self.agent.workspace_degrades,
		)
		.await
		.map_err(Error::from)?;
		if self.remote.is_some() {
			return Ok(semantic);
		}
		let reserved =
			serde_json::to_vec(&serde_json::json!({"workspace":semantic,"memory":null}))?.len();
		let available = budget.saturating_sub(reserved);
		let mut access = self.access.write().await;
		let memory = crate::semantic::services::memory_context::retrieve(
			store,
			&mut crate::semantic::service::Lease::Inherited(&mut access),
			&self.run,
			task,
			inputs,
			available,
			&self.agent,
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
			if request.run_id != self.run.id || request.workspace_id != self.run.workspace_id {
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

	/// The compaction Decider a remote or generated Run must use instead of its
	/// Agent's Binding: the Decider pinned for this executing Node, or the one
	/// every generated ancestor's policy names (ADR 0042).
	pub(crate) async fn chain_decider(
		&self,
		f: &Federation,
	) -> aidash_application::Result<aidash_application::generation::compaction::Chain> {
		use aidash_application::generation::compaction::{self, Chain, remote};
		let mut access = self.access.write().await;
		if let Some(federation) = &self.remote {
			let mut scope =
				crate::bootstrap::generation_remote_compaction_scope(&mut access, federation);
			return Ok(Chain::Generated(
				remote::decider(&mut scope, &self.run).await?.map(Box::new),
			));
		}
		compaction::decider(
			&mut crate::bootstrap::generation_compaction_authority_scope(&mut access),
			f.store.deployment.as_str(),
			&self.generation_context(),
		)
		.await
	}

	/// Charge the owners beside the Run itself for one Decision Attempt: the
	/// Home of a remote Run, or every generated ancestor. Returns their receipts
	/// and, for a remote Run, the Home's open reservation to settle once the
	/// attempt's result is kept.
	pub(crate) async fn charge_decision(
		&self,
		f: &Federation,
		decider: &EntityRef,
		config: &aidash_domain::decision::DeciderConfig,
		attempt: Uuid,
		request_digest: &str,
		questions: usize,
	) -> aidash_application::Result<(
		std::collections::BTreeMap<String, Uuid>,
		Option<aidash_domain::generation::dispatch::Input>,
	)> {
		use aidash_application::generation::compaction::{self, remote};
		let mut access = self.access.write().await;
		if let Some(federation) = &self.remote {
			let mut scope =
				crate::bootstrap::generation_remote_compaction_scope(&mut access, federation);
			let reservation = remote::admit(
				&mut scope,
				&self.run,
				attempt,
				request_digest.to_owned(),
				config,
			)
			.await?;
			// The Home correlates its reservation by the Decision Attempt, so
			// that, not the shared remote Run, is the receipt.
			return Ok((
				std::collections::BTreeMap::from([(
					format!("home/runs/{}", self.run.id),
					reservation.usage.attempt_id,
				)]),
				Some(reservation),
			));
		}
		let tenancy = access.tenancy()?;
		let bytes = i64::try_from(config.max_request_bytes()).map_err(|_| {
			aidash_application::Error::Invalid("decision request bound overflow".into())
		})?;
		let questions = i32::try_from(questions).map_err(|_| {
			aidash_application::Error::Invalid("too many decision questions".into())
		})?;
		let receipts = compaction::charge(
			&mut crate::bootstrap::generation_compaction_authority_scope(&mut access),
			&crate::bootstrap::generation_compaction_repository(&f.store, tenancy),
			&self.generation_context(),
			decider,
			attempt,
			bytes,
			questions,
		)
		.await?;
		Ok((receipts, None))
	}

	/// Settle the Home's reservation of a remote Decision Attempt.
	pub(crate) async fn settle_decision(
		&self,
		reservation: &aidash_domain::generation::dispatch::Input,
	) -> aidash_application::Result<()> {
		let federation = self
			.remote
			.as_ref()
			.ok_or(aidash_application::Error::Forbidden)?;
		let mut access = self.access.write().await;
		let mut scope =
			crate::bootstrap::generation_remote_compaction_scope(&mut access, federation);
		aidash_application::generation::compaction::remote::settle(
			&mut scope,
			&self.run,
			reservation,
		)
		.await
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

	/// Authorize the exact pinned summarizer and charge every allowance
	/// before the Summary Stage request leaves this node.
	pub async fn reserve_summary(
		&self,
		f: &Federation,
		attempt: Uuid,
		summarizer: &aidash_domain::context::summary::SummaryProvider,
		window: usize,
		output: u32,
		request: &crate::provider::ModelRequest,
	) -> Result<Option<crate::generation::budget::InferenceReservation>> {
		let request_bytes = serde_json::to_vec(request)?.len().max(1) as i64;
		aidash_application::execution::summary::authorize(
			&SummaryApproval { guard: self, f },
			summarizer,
			request_bytes,
		)
		.await?;
		aidash_application::execution::admission::reserve(
			&crate::bootstrap::summary_admission_repository(
				&f.store,
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
		.map_err(aidash_application::execution::summary::classify)
		.map_err(Into::into)
	}

	/// Repeat every `reserve_summary` approval for the exact summarizer without
	/// charging another call; a revoked summarizer's output is never adopted.
	pub async fn recheck_summary(
		&self,
		f: &Federation,
		summarizer: &aidash_domain::context::summary::SummaryProvider,
	) -> Result<()> {
		aidash_application::execution::summary::recheck(
			&SummaryApproval { guard: self, f },
			summarizer,
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
			self.run.workspace_id,
			ids.iter().copied(),
		)
		.await
		.map_err(Into::into)
	}

	pub async fn inference(&self) -> Result<()> {
		self.refresh_remote().await?;
		let mut access = self.access.write().await;
		self.authorize_inference_with(&mut access).await
	}

	/// Resolve an explicit one-inference file selection or page image under the
	/// same execution authority as the provider request. Only immutable
	/// references are durable.
	pub async fn model_media(
		&self,
		store: &Store,
		selections: &[aidash_domain::media::ModelMedia],
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
		let mut access = self.access.write().await;
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

	pub async fn finish(self, result: Result<()>) -> Result<()> {
		let access = Arc::try_unwrap(self.access)
			.map_err(|_| Error::Conflict("execution boundary still in use".into()))?
			.into_inner();
		access.finish(result).await
	}
}

/// Summary Stage approvals under the Guard's retained worker authority.
struct SummaryApproval<'a> {
	guard: &'a Guard,
	f: &'a Federation,
}
#[async_trait::async_trait]
impl aidash_application::ports::execution::summary::SummaryAuthorization for SummaryApproval<'_> {
	fn remote(&self) -> bool {
		self.guard.remote.is_some()
	}
	fn node_id(&self) -> &str {
		self.f.config.deployment.as_str()
	}
	async fn refresh(&self) -> aidash_application::Result<()> {
		self.guard.refresh_remote().await.map_err(Into::into)
	}
	async fn remote_binding(
		&self,
	) -> aidash_application::Result<aidash_domain::semantic::remote::Binding> {
		use aidash_application::ports::generation::compaction::remote::RemoteCompactionAuthority as _;
		let remote = self.guard.remote.as_ref().ok_or(Error::Forbidden)?;
		let mut access = self.guard.access.write().await;
		let mut scope = crate::bootstrap::generation_remote_compaction_scope(&mut access, remote);
		// Reacquire the Home's current lease before reading its disclosed pins.
		scope.suspend().await?;
		if !scope.refresh(&self.guard.run).await? {
			return Err(aidash_application::Error::Forbidden);
		}
		let description: aidash_domain::federation::execution::Description =
			serde_json::from_value(scope.description(self.guard.run.id).await?)?;
		Ok(description.semantic)
	}
	async fn catalog_entry(
		&self,
		model: &EntityRef,
		action: &str,
	) -> aidash_application::Result<crate::registry::Entry> {
		let mut access = self.guard.access.write().await;
		catalog::entry(&mut access, model, action)
			.await
			.map_err(Into::into)
	}
	async fn charge_generated(
		&self,
		summarizer: &aidash_domain::context::summary::SummaryProvider,
		request_bytes: i64,
	) -> aidash_application::Result<()> {
		let mut access = self.guard.access.write().await;
		let tenancy = access.tenancy()?;
		aidash_application::generation::summary::reserve(
			&mut crate::bootstrap::generation_compaction_authority_scope(&mut access),
			&crate::bootstrap::generation_summary_repository(&self.f.store, tenancy),
			&self.generation_context(),
			summarizer,
			request_bytes,
		)
		.await
		.map(|_| ())
	}
	async fn recheck_generated(
		&self,
		summarizer: &aidash_domain::context::summary::SummaryProvider,
	) -> aidash_application::Result<()> {
		let mut access = self.guard.access.write().await;
		aidash_application::generation::summary::recheck(
			&mut crate::bootstrap::generation_compaction_authority_scope(&mut access),
			self.f.store.deployment.as_str(),
			&self.generation_context(),
			summarizer,
		)
		.await
	}
}
impl Guard {
	fn generation_context(&self) -> aidash_domain::generation::compaction::Context {
		let run = &self.run;
		aidash_domain::generation::compaction::Context {
			run: run.id,
			task: run.task_id,
			agent: EntityRef {
				id: run.agent_id.clone(),
				version: run.agent_version.clone(),
			},
		}
	}
}
impl SummaryApproval<'_> {
	fn generation_context(&self) -> aidash_domain::generation::compaction::Context {
		let run = &self.guard.run;
		aidash_domain::generation::compaction::Context {
			run: run.id,
			task: run.task_id,
			agent: EntityRef {
				id: run.agent_id.clone(),
				version: run.agent_version.clone(),
			},
		}
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
