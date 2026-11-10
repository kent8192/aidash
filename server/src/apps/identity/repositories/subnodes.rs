//! Run-scoped Subnode creation under the caller's retained authority.
use crate::{
	apps::{
		execution::models::subnodes::{SubnodeLimitRecord, SubnodeRecord},
		identity::services::access::Access,
	},
	federation::Federation,
};
use aidash_application::{
	Result,
	authorization::Snapshot,
	ports::{authorization::lease::AuthorizationLease, subnodes::CreateScope},
	subnodes::Create,
};
use aidash_domain::{
	RunMetadata,
	policy::{Decision, Evaluation, Resource},
	registry::{EntityRef, bindings::BindingSnapshot},
	subnodes::{Allowance, Member, State, Subnode, SubnodeId},
};
use async_trait::async_trait;
use chrono::Utc;
use reinhardt::{
	db::orm::{Model, execution::convert_values},
	query::{
		Alias, ColumnRef, Expr, ExprTrait, LockType, PostgresQueryBuilder, Query,
		QueryStatementBuilder,
	},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use uuid::Uuid;

/// The snapshot is a verified, pinned closure; callers must reject member
/// plugins because their tools do not have stable allowance identities.
pub(crate) fn binding_authority(
	snapshot: &BindingSnapshot,
	config: &aidash_domain::registry::AgentConfig,
) -> Result<Allowance> {
	let mut authority = Allowance::default();
	for binding in &snapshot.bindings {
		if binding.excluded_reason.is_some()
			|| binding.definition.kind != "tool"
			|| !creator_can_use_binding(config, binding)?
		{
			continue;
		}
		let name = if binding.definition.config.get("provider").is_some() {
			let descriptor: aidash_domain::tool::providers::ToolDescriptor =
				serde_json::from_value(binding.definition.config.clone())?;
			if descriptor.provider.starts_with("core.") {
				descriptor.operation
			} else {
				binding.identity.resource_id()
			}
		} else {
			binding.identity.resource_id()
		};
		if !aidash_domain::subnodes::COORDINATION_TOOLS.contains(&name.as_str()) {
			authority.tools.insert(name);
		}
		if let Some(secret) = aidash_domain::tenant_secrets::definition_secret(&binding.definition)
		{
			authority.secrets.insert(secret);
		}
	}
	Ok(authority)
}

/// Every usable member Binding must be covered by a creator Binding with the
/// same pinned definition and at least as much effective authority.
pub(crate) fn check_member_bindings(
	creator: &BindingSnapshot,
	member: &BindingSnapshot,
	config: &aidash_domain::registry::AgentConfig,
) -> Result<()> {
	for binding in &member.bindings {
		if binding.excluded_reason.is_some()
			|| binding.definition.kind != "tool"
			|| !creator_can_use_binding(config, binding)?
		{
			continue;
		}
		if let Ok(descriptor) = serde_json::from_value::<
			aidash_domain::tool::providers::ToolDescriptor,
		>(binding.definition.config.clone())
			&& descriptor.provider.starts_with("core.")
			&& aidash_domain::subnodes::COORDINATION_TOOLS.contains(&descriptor.operation.as_str())
		{
			continue;
		}
		let covered = creator.bindings.iter().any(|parent| {
			parent.identity == binding.identity
				&& parent.digest == binding.digest
				&& parent
					.narrow
					.intersect(&binding.narrow)
					.is_ok_and(|intersection| intersection == binding.narrow)
		});
		if !covered {
			return Err(aidash_application::Error::Refused(format!(
				"member Binding {} exceeds creator restrictions",
				binding.identity.resource_id()
			)));
		}
	}
	Ok(())
}

/// A pinned Binding is not usable when the creator lacks its Agent or core
/// capability prerequisite, even if current policy allows tool.invoke.
pub(crate) fn creator_can_use_binding(
	config: &aidash_domain::registry::AgentConfig,
	binding: &aidash_domain::registry::bindings::ResolvedBinding,
) -> Result<bool> {
	if binding.definition.config.get("provider").is_none() {
		return Ok(true);
	}
	let descriptor: aidash_domain::tool::providers::ToolDescriptor =
		serde_json::from_value(binding.definition.config.clone())?;
	let contract = descriptor.declared_contract(binding.identity.clone())?;
	Ok(contract
		.authorization
		.flag
		.is_none_or(|flag| flag.permitted(config))
		&& contract
			.authorization
			.core
			.is_none_or(|core| core.permitted(&config.core_capabilities)))
}

pub(crate) async fn subnode_scratch_binding(
	access: &mut Access,
	run: &RunMetadata,
	policy: &crate::config::ScratchOperatorPolicy,
) -> Result<(Uuid, String)> {
	let mut services =
		crate::apps::workspaces::repositories::scratch::NativeScratchServices { policy };
	services.run_binding_metadata(access, run).await
}

pub(crate) struct Creation<'a> {
	pub access: &'a mut Access,
	pub federation: &'a Federation,
	pub node_address: String,
	pub remote: bool,
	pub creator_authority: Allowance,
	pub members: BTreeMap<String, Allowance>,
}
#[async_trait]
impl AuthorizationLease for Creation<'_> {
	fn snapshot(&self) -> &Snapshot {
		&self.access.snapshot
	}
	fn subjects(&self) -> &[String] {
		&self.access.subjects
	}
	fn environment(&self) -> &Value {
		self.access.environment()
	}
	async fn record(&mut self, records: &[(Evaluation, Decision)]) -> Result<()> {
		self.access.record(records).await.map_err(Into::into)
	}
}
#[async_trait]
impl CreateScope for Creation<'_> {
	fn execution_node(&self) -> &str {
		&self.node_address
	}
	async fn node_resource(&mut self, run: &RunMetadata) -> Result<Resource> {
		self.access
			.node_resource(&run.node)
			.await
			.map_err(Into::into)
	}
	async fn workspace_is_subnode(&mut self, workspace: Uuid) -> Result<bool> {
		let tenant = self.access.identity.tenant.clone();
		Ok(!SubnodeRecord::objects()
			.filter(SubnodeRecord::field_tenant().eq(tenant))
			.filter(SubnodeRecord::field_workspace_id().eq(workspace))
			.all_with_executor(self.access.tx.native_mut())
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(crate::Error::from)?
			.is_empty())
	}
	async fn remote_grant_started_run(&mut self, _: Uuid) -> Result<bool> {
		Ok(self.remote)
	}
	async fn replay(
		&mut self,
		run: &RunMetadata,
		key: &str,
		input: &Create,
	) -> Result<Option<Subnode>> {
		// Lock the creator first, fencing terminal transition and parallel creates.
		let (sql, values) = Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("runs"))
			.and_where(Expr::col("id").eq(Expr::value(run.id)))
			.lock(LockType::NoKeyUpdate)
			.build(PostgresQueryBuilder);
		let row = self
			.access
			.tx
			.native_mut()
			.fetch_optional(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from)?
			.ok_or(aidash_application::Error::Forbidden)?;
		let row = reinhardt::db::orm::QueryRow::from_backend_row(row);
		let phase: String = row.get("phase").ok_or(crate::Error::Forbidden)?;
		let control: String = row.get("control").ok_or(crate::Error::Forbidden)?;
		if matches!(phase.as_str(), "COMPLETED" | "FAILED" | "CANCELLED") || control == "CANCELLED"
		{
			return Err(aidash_application::Error::Refused(
				"creating Run has ended".into(),
			));
		}
		let fingerprint = crate::registry::digest(&serde_json::to_value(input)?);
		let tenant = self.access.identity.tenant.clone();
		let row = SubnodeRecord::by_key(self.access.tx.native_mut(), &tenant, run.id, key).await?;
		match row {
			Some(row) if row.creation_fingerprint != fingerprint => {
				Err(aidash_application::Error::Conflict(
					"Subnode creation key was reused with different input".into(),
				))
			}
			Some(row) => Ok(Some(row.domain(self.access.tx.native_mut()).await?)),
			None => Ok(None),
		}
	}
	async fn limits(&mut self) -> Result<aidash_domain::subnodes::SubnodeLimits> {
		let tenant = self.access.identity.tenant.clone();
		SubnodeLimitRecord::get(self.access.tx.native_mut(), &tenant)
			.await
			.map_err(Into::into)
	}
	async fn active_count(&mut self, run: Uuid) -> Result<usize> {
		let tenant = self.access.identity.tenant.clone();
		SubnodeRecord::active_count(self.access.tx.native_mut(), &tenant, run)
			.await
			.map_err(Into::into)
	}
	async fn creator_authority(&mut self, _: &RunMetadata) -> Result<Allowance> {
		Ok(self.creator_authority.clone())
	}
	async fn approved_member_authority(&mut self, agent: &EntityRef) -> Result<Allowance> {
		self.members
			.get(&format!("{}@{}", agent.id, agent.version))
			.cloned()
			.ok_or(aidash_application::Error::Forbidden)
	}
	async fn insert(&mut self, run: &RunMetadata, key: &str, input: &Create) -> Result<Subnode> {
		let tenant = self.access.identity.tenant.clone();
		let workspace_id = Uuid::new_v4();
		let title = input.title.as_deref().unwrap_or("Subnode");
		let (scratch_space_id, created_by) =
			subnode_scratch_binding(self.access, run, &self.federation.config.scratch_policy)
				.await?;
		let workspace = self
			.federation
			.store
			.create_workspace_in(
				&mut self.access.tx,
				crate::apps::execution::repositories::store::WorkspaceInsert {
					id: workspace_id,
					tenant: &tenant,
					node: &run.node,
					title,
					goal: title,
					scratch_space_id,
					created_by: &created_by,
				},
			)
			.await?;
		self.federation
			.store
			.record_workspace_tenant_in(
				&mut self.access.tx,
				workspace.id,
				&tenant,
				&self.access.identity.subject,
			)
			.await?;
		let now = Utc::now();
		let value = Subnode {
			id: SubnodeId(Uuid::new_v4()),
			tenant,
			node: self.node_address.clone(),
			creator_run_id: run.id,
			creator_task_id: run.task_id,
			creator_agent: EntityRef {
				id: run.agent_id.clone(),
				version: run.agent_version.clone(),
			},
			leader: input.leader.clone(),
			members: input
				.members
				.iter()
				.map(|agent| Member {
					agent_id: agent.id.clone(),
					agent_version: agent.version.clone(),
					added_by_run_id: run.id,
					added_at: now,
				})
				.collect(),
			allowance: input.allowance.clone(),
			workspace_id,
			title: input.title.clone(),
			state: State::Active,
			created_at: now,
			ended_at: None,
			ended_reason: None,
		};
		let fingerprint = crate::registry::digest(&serde_json::to_value(input)?);
		SubnodeRecord::insert_active(self.access.tx.native_mut(), &value, key, &fingerprint)
			.await?;
		self.federation
			.store
			.event(
				&mut self.access.tx,
				Some(workspace_id),
				"subnode.created",
				json!({"subnode_id":value.id.0,"creator_run_id":run.id}),
			)
			.await?;
		Ok(value)
	}
}

/// All management mutations lock creator Run first, then Subnode, matching the
/// terminal and cancellation cascade's lock order.
pub(crate) struct Management<'a> {
	pub access: &'a mut Access,
	pub federation: &'a Federation,
	pub creator_authority: Allowance,
	pub candidate_authority: Option<Allowance>,
	pub message_limit: usize,
}
#[async_trait]
impl aidash_application::ports::subnodes::ManageScope for Management<'_> {
	async fn lock(&mut self, id: SubnodeId) -> Result<Option<Subnode>> {
		let tenant = self.access.identity.tenant.clone();
		let record = SubnodeRecord::objects()
			.filter(SubnodeRecord::field_tenant().eq(tenant.clone()))
			.filter(SubnodeRecord::field_id().eq(id.0))
			.all_with_executor(self.access.tx.native_mut())
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(crate::Error::from)?
			.pop();
		let Some(record) = record else {
			return Ok(None);
		};
		let (sql, values) = Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("runs"))
			.and_where(Expr::col("id").eq(Expr::value(record.creator_run_id)))
			.lock(LockType::NoKeyUpdate)
			.build(PostgresQueryBuilder);
		if self
			.access
			.tx
			.native_mut()
			.fetch_optional(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from)?
			.is_none()
		{
			return Err(aidash_application::Error::Forbidden);
		}
		let (sql, values) = Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("subnodes"))
			.and_where(Expr::col("tenant").eq(tenant))
			.and_where(Expr::col("id").eq(Expr::value(id.0)))
			.lock(LockType::Update)
			.build(PostgresQueryBuilder);
		let Some(row) = self
			.access
			.tx
			.native_mut()
			.fetch_optional(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from)?
		else {
			return Ok(None);
		};
		let record: SubnodeRecord =
			serde_json::from_value(reinhardt::db::orm::QueryRow::from_backend_row(row).data)?;
		Ok(Some(record.domain(self.access.tx.native_mut()).await?))
	}
	async fn limits(&mut self, tenant: &str) -> Result<aidash_domain::subnodes::SubnodeLimits> {
		SubnodeLimitRecord::get(self.access.tx.native_mut(), tenant)
			.await
			.map_err(Into::into)
	}
	async fn creator_authority(&mut self, _: &Subnode) -> Result<Allowance> {
		Ok(self.creator_authority.clone())
	}
	async fn approved_member_authority(&mut self, _: &Subnode, _: &EntityRef) -> Result<Allowance> {
		self.candidate_authority
			.clone()
			.ok_or(aidash_application::Error::Forbidden)
	}
	async fn member_has_active_work(
		&mut self,
		subnode: &Subnode,
		agent: &EntityRef,
	) -> Result<bool> {
		let (sql, values) = Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("runs"))
			.and_where(Expr::col("workspace_id").eq(Expr::value(subnode.workspace_id)))
			.and_where(Expr::col("agent_id").eq(agent.id.clone()))
			.and_where(Expr::col("agent_version").eq(agent.version.clone()))
			.and_where(Expr::col("phase").is_not_in(["COMPLETED", "FAILED", "CANCELLED"]))
			.limit(1)
			.build(PostgresQueryBuilder);
		if self
			.access
			.tx
			.native_mut()
			.fetch_optional(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from)?
			.is_some()
		{
			return Ok(true);
		}
		// A task can remain assigned even after its Run becomes terminal, or
		// while a delegation is recorded but has not yet produced a Run.
		let delegated = Query::select()
			.column(Alias::new("task_id"))
			.from(Alias::new("delegations"))
			.and_where(Expr::col("agent_id").eq(agent.id.clone()))
			.and_where(Expr::col("agent_version").eq(agent.version.clone()))
			.to_owned();
		let subject = aidash_domain::agent_subject(&subnode.node, &agent.id, &agent.version);
		let (sql, values) = Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("tasks"))
			.and_where(Expr::col("workspace_id").eq(Expr::value(subnode.workspace_id)))
			.and_where(Expr::col("status").is_not_in([
				"COMPLETED",
				"FAILED",
				"CANCELLED",
				"ABANDONED",
			]))
			.and_where(
				Expr::col("owner")
					.eq(subject)
					.or(Expr::col("id").in_subquery(delegated)),
			)
			.limit(1)
			.build(PostgresQueryBuilder);
		Ok(self
			.access
			.tx
			.native_mut()
			.fetch_optional(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from)?
			.is_some())
	}
	async fn add_member(
		&mut self,
		subnode: &Subnode,
		agent: &EntityRef,
		caller: Uuid,
	) -> Result<()> {
		use crate::apps::execution::models::subnodes::SubnodeMemberRecord;
		let row = SubnodeMemberRecord {
			subnode_id: subnode.id.0,
			tenant: subnode.tenant.clone(),
			agent_id: agent.id.clone(),
			agent_version: agent.version.clone(),
			added_by_run_id: caller,
			added_at: Utc::now(),
		};
		SubnodeMemberRecord::objects()
			.insert_with_executor(self.access.tx.native_mut(), &row)
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(crate::Error::from)?;
		self.federation
			.store
			.event(
				&mut self.access.tx,
				Some(subnode.workspace_id),
				"subnode.member_added",
				json!({"subnode_id":subnode.id.0,"agent":agent}),
			)
			.await?;
		Ok(())
	}
	async fn remove_member(&mut self, subnode: &Subnode, agent: &EntityRef) -> Result<()> {
		let (sql, values) = Query::delete()
			.from_table(Alias::new("subnode_members"))
			.and_where(Expr::col("tenant").eq(subnode.tenant.clone()))
			.and_where(Expr::col("subnode_id").eq(Expr::value(subnode.id.0)))
			.and_where(Expr::col("agent_id").eq(agent.id.clone()))
			.and_where(Expr::col("agent_version").eq(agent.version.clone()))
			.build(PostgresQueryBuilder);
		self.access
			.tx
			.native_mut()
			.execute(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from)?;
		self.federation
			.store
			.event(
				&mut self.access.tx,
				Some(subnode.workspace_id),
				"subnode.member_removed",
				json!({"subnode_id":subnode.id.0,"agent":agent}),
			)
			.await?;
		Ok(())
	}
	async fn task(
		&mut self,
		subnode: &Subnode,
		run: &RunMetadata,
		key: &str,
		title: &str,
		request: &str,
		journal_range: Option<aidash_domain::subnodes::JournalRange>,
	) -> Result<Uuid> {
		use aidash_application::ports::authorization::worker_tasks::WorkerTaskScope;
		if let Some(range) = journal_range {
			if range.from_seq > range.through_seq || range.through_seq > i64::MAX as u64 {
				return Err(aidash_application::Error::Invalid(
					"invalid journal range".into(),
				));
			}
			let count: i64 = crate::database::native::query_scalar(
				&Query::select()
					.expr(Expr::cust("COUNT(*)"))
					.from(Alias::new("run_context_events"))
					.and_where(Expr::col("run_id").eq(Expr::value(run.id)))
					.and_where(
						Expr::col("seq").is_in([range.from_seq as i64, range.through_seq as i64]),
					)
					.to_string(PostgresQueryBuilder),
			)
			.scalar_one(&mut **self.access.tx.native_mut())
			.await?;
			let expected = if range.from_seq == range.through_seq {
				1
			} else {
				2
			};
			if count != expected {
				return Err(aidash_application::Error::Invalid(
					"journal range is outside the creating Run's Context Journal".into(),
				));
			}
		}
		// tasks_parent_workspace requires a same-Workspace parent. Provenance
		// instead records the creator Run in authorization_task_origins.
		let input = aidash_domain::NewTask {
			title: title.into(),
			description: request.into(),
			requirements: json!({}),
			dependencies: vec![],
			parent_id: None,
		};
		self.access.context = json!({"workspace_id":subnode.workspace_id});
		let workspace = self.access.workspace(subnode.workspace_id).await?;
		self.access.require(&workspace, "task.create").await?;
		let creator = self
			.access
			.subjects
			.last()
			.cloned()
			.ok_or(aidash_application::Error::Forbidden)?;
		let task = self
			.federation
			.store
			.create_task_in(
				&mut self.access.tx,
				subnode.workspace_id,
				&input,
				&creator,
				Some(key),
			)
			.await?;
		if let Some(range) = journal_range {
			let (sql, values) = Query::update()
				.table(Alias::new("tasks"))
				.value_expr(
					Alias::new("journal_range"),
					Expr::value(serde_json::to_value(range)?),
				)
				.and_where(Expr::col("id").eq(Expr::value(task.id)))
				.build(PostgresQueryBuilder);
			self.access
				.tx
				.as_mut()
				.execute(&sql, convert_values(values))
				.await
				.map_err(crate::Error::from)?;
			crate::apps::identity::models::AuthorizationRunRead::record_sources(
				self.access.tx.as_mut(),
				run.id,
				run.workspace_id,
				&[("run".into(), run.id)],
			)
			.await?;
		}
		let mut scope = super::worker_tasks::WorkerTasks {
			federation: self.federation,
			access: &mut *self.access,
		};
		scope.insert_origin(task.id, run.id).await?;
		let actual = scope.created_origin(task.id).await?;
		let identity = scope.identity();
		if actual.source_run_id != run.id
			|| actual.authority.tenant != identity.tenant
			|| actual.authority.root_subject != identity.subject
			|| actual.authority.subject_chain != scope.subjects()
		{
			// The call caused this, as for `task_create`: the model hears it instead of the Run
			// failing, and a new call creates a new task.
			return Err(aidash_application::Error::Refused(
				"a task from an earlier attempt of this call has a different origin, so it cannot be reused; calling subnode_task again creates a new task".into(),
			));
		}
		let resource = scope.task_resource(&task).await?;
		scope.require(&resource, "task.read").await?;
		scope.require(&resource, "task.delegate").await?;
		if let Some(existing) = scope.execution_grant(task.id).await? {
			let mut chain = scope.subjects().to_vec();
			chain.push(aidash_domain::agent_subject(
				&subnode.node,
				&subnode.leader.id,
				&subnode.leader.version,
			));
			if existing.subject_chain != chain || existing.credential_id != identity.credential_id {
				return Err(aidash_application::Error::Refused(
					"the task was already delegated under a different authority".into(),
				));
			}
		} else {
			scope.delegate(task.id, &subnode.leader).await?;
		}
		self.federation
			.store
			.record_output_in(
				&mut self.access.tx,
				Some(run.id),
				subnode.workspace_id,
				"task",
				task.id,
			)
			.await?;
		Ok(task.id)
	}
	async fn dissolve(
		&mut self,
		subnode: &Subnode,
		reason: aidash_domain::subnodes::EndReason,
	) -> Result<()> {
		use aidash_application::ports::subnodes::EndScope;
		let mut cascade = crate::apps::execution::repositories::subnodes::Cascade {
			store: &self.federation.store,
			tx: self.access.tx.native_mut(),
		};
		// Lock the Runs before `mark_ended` appends an event; see `lock_end`.
		let runs = cascade.active_runs(subnode.workspace_id).await?;
		cascade
			.mark_ended(subnode.id, subnode.workspace_id, reason)
			.await?;
		for run in runs {
			cascade
				.control_run(
					run,
					aidash_domain::RunControlAction::Cancel,
					aidash_domain::ControlSource::System,
				)
				.await?;
		}
		Ok(())
	}
	async fn message_creator(
		&mut self,
		subnode: &Subnode,
		run: &RunMetadata,
		key: &str,
		content: &str,
	) -> Result<()> {
		self.federation
			.store
			.accept_member_message_in(
				self.access.tx.native_mut(),
				subnode.creator_run_id,
				run,
				content,
				key,
				self.message_limit,
			)
			.await?;
		Ok(())
	}
}

/// Recheck the creating Run's own pinned tools under its exact subject chain.
/// Restoring the caller's authority on Drop also covers cancellation.
struct CreatorView<'a> {
	access: &'a mut Access,
	subjects: Vec<String>,
	approved: std::collections::BTreeSet<(String, String)>,
	context: Value,
}
impl std::ops::Deref for CreatorView<'_> {
	type Target = Access;
	fn deref(&self) -> &Access {
		self.access
	}
}
impl std::ops::DerefMut for CreatorView<'_> {
	fn deref_mut(&mut self) -> &mut Access {
		self.access
	}
}
impl Drop for CreatorView<'_> {
	fn drop(&mut self) {
		self.access.subjects = std::mem::take(&mut self.subjects);
		self.access.approved_catalog = std::mem::take(&mut self.approved);
		self.access.context = std::mem::take(&mut self.context);
	}
}
pub(crate) async fn creator_binding_authority(
	access: &mut Access,
	creator: &aidash_domain::Run,
	subnode: &Subnode,
) -> crate::Result<(Allowance, BindingSnapshot)> {
	let snapshot = creator
		.context
		.binding_snapshot
		.as_ref()
		.ok_or_else(|| crate::Error::Invalid("creating Run has no Binding snapshot".into()))?;
	let subject = aidash_domain::agent_subject(
		&subnode.node,
		&subnode.creator_agent.id,
		&subnode.creator_agent.version,
	);
	let position = access
		.subjects
		.iter()
		.rposition(|entry| entry == &subject)
		.ok_or(crate::Error::Forbidden)?;
	let subjects = access.subjects[..=position].to_vec();
	let approved = snapshot
		.definitions
		.iter()
		.map(|entry| (entry.identity.id.clone(), entry.identity.version.clone()))
		.collect();
	let mut view = CreatorView {
		subjects: std::mem::replace(&mut access.subjects, subjects),
		approved: std::mem::replace(&mut access.approved_catalog, approved),
		context: std::mem::replace(
			&mut access.context,
			json!({"workspace_id":creator.workspace_id}),
		),
		access,
	};
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
		match crate::apps::identity::services::catalog::tool_entry(
			&mut view,
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
				match crate::marketplace::check_pinned(&mut view, &entry).await {
					Ok(()) => usable.bindings.push(binding.clone()),
					Err(
						crate::Error::Forbidden
						| crate::Error::NotFound(_)
						| crate::Error::AuthorityRefused(_),
					) => {}
					Err(error) => return Err(error),
				}
			}
			Err(crate::Error::Forbidden)
			| Err(crate::Error::NotFound(_))
			| Err(crate::Error::AuthorityRefused(_)) => {}
			Err(error) => return Err(error),
		}
	}
	Ok((binding_authority(&usable, &config)?, usable))
}

/// A Task in a Subnode can only originate from its creator or a current member,
/// and may only run as a current member. The row lock serializes admission with
/// removal and dissolution, including HTTP claims outside the worker path.
pub(crate) async fn require_admission(
	access: &mut Access,
	task: &aidash_domain::Task,
	target: &EntityRef,
) -> crate::Result<()> {
	use crate::apps::execution::models::subnodes::SubnodeMemberRecord;
	let tenant = access.identity.tenant.clone();
	let rows = SubnodeRecord::objects()
		.filter(SubnodeRecord::field_tenant().eq(tenant.clone()))
		.filter(SubnodeRecord::field_workspace_id().eq(task.workspace_id))
		.select_for_update()
		.all_with_executor(access.tx.native_mut())
		.await
		.map_err(reinhardt::core::exception::Error::from)?;
	let Some(subnode) = rows.into_iter().next() else {
		return Ok(());
	};
	if subnode.state != "active" {
		return Err(crate::Error::Forbidden);
	}
	let members = SubnodeMemberRecord::objects()
		.filter(SubnodeMemberRecord::field_tenant().eq(tenant))
		.filter(SubnodeMemberRecord::field_subnode_id().eq(subnode.id))
		.all_with_executor(access.tx.native_mut())
		.await
		.map_err(reinhardt::core::exception::Error::from)?;
	if !members
		.iter()
		.any(|member| member.agent_id == target.id && member.agent_version == target.version)
	{
		return Err(crate::Error::Forbidden);
	}
	let (sql, values) = Query::select()
		.column(Alias::new("source_run_id"))
		.from(Alias::new("authorization_task_origins"))
		.and_where(Expr::col("task_id").eq(Expr::value(task.id)))
		.build(PostgresQueryBuilder);
	let Some(row) = access
		.tx
		.native_mut()
		.fetch_optional(&sql, convert_values(values))
		.await?
	else {
		return Err(crate::Error::Forbidden);
	};
	let source: Uuid = reinhardt::db::orm::QueryRow::from_backend_row(row)
		.get("source_run_id")
		.ok_or(crate::Error::Forbidden)?;
	if source == subnode.creator_run_id {
		return Ok(());
	}
	let (sql, values) = Query::select()
		.columns(["agent_id", "agent_version", "workspace_id"].map(Alias::new))
		.from(Alias::new("runs"))
		.and_where(Expr::col("id").eq(Expr::value(source)))
		.build(PostgresQueryBuilder);
	let Some(row) = access
		.tx
		.native_mut()
		.fetch_optional(&sql, convert_values(values))
		.await?
	else {
		return Err(crate::Error::Forbidden);
	};
	let row = reinhardt::db::orm::QueryRow::from_backend_row(row);
	let workspace: Uuid = row.get("workspace_id").ok_or(crate::Error::Forbidden)?;
	let id: String = row.get("agent_id").ok_or(crate::Error::Forbidden)?;
	let version: String = row.get("agent_version").ok_or(crate::Error::Forbidden)?;
	if workspace != subnode.workspace_id
		|| !members
			.iter()
			.any(|member| member.agent_id == id && member.agent_version == version)
	{
		return Err(crate::Error::Forbidden);
	}
	Ok(())
}
