use crate::apps::federation::remote::repositories::routes;
use crate::{
	Error, Result,
	config::Config,
	domain::*,
	registry::{EntityRef, Entry, Registry, Search},
	store::{RunResponseMessage, Store},
};
use aidash_application::tenancy::TenancyScope;
use aidash_domain::federation::route::{Route, Sender};
use reinhardt::DiError;
use reinhardt::DiResult;
use reinhardt::Injectable;
use reinhardt::InjectionContext;
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Clone)]
pub struct Federation {
	pub sandbox: aidash_runtime::sandbox::Sessions,
	pub store: Store,
	pub registry: Registry,
	pub config: Config,
	pub client: reqwest::Client,
	pub notify: std::sync::Arc<tokio::sync::Notify>,
}
impl Federation {
	pub async fn admit_run_message(
		&self,
		run: &Run,
		sender: &str,
		content: &str,
		key: &str,
		limit: usize,
	) -> Result<()> {
		aidash_application::federation::run_messages::admit(
			&crate::bootstrap::execution_message_scope(self, run),
			sender,
			content,
			key,
			limit,
		)
		.await
		.map_err(Into::into)
	}
	pub async fn acknowledge_run_messages(&self, run: &Run) -> Result<()> {
		aidash_application::federation::run_messages::acknowledge_observed(
			&crate::bootstrap::run_message_scope(self, &run.metadata()),
		)
		.await
		.map_err(Into::into)
	}
	/// Explicit cancellation or failure supersedes any still-unobserved
	/// correction. Consume the home fences in the same task-row transaction as
	/// the terminal transition so a legacy completion cannot enter between them.
	pub async fn transition_terminal_run_messages(
		&self,
		run: &Run,
		target: TaskStatus,
	) -> Result<Task> {
		self.transition_terminal_metadata(&run.metadata(), target, None)
			.await
	}
	pub async fn require_terminal_safe_delivery(&self, run: &Run) -> Result<()> {
		aidash_application::federation::run_messages::require_terminal_safe_delivery(
			&crate::bootstrap::run_message_scope(self, &run.metadata()),
		)
		.await
		.map_err(Into::into)
	}
	pub async fn run_message_limit(&self, run: &Run) -> Result<usize> {
		aidash_application::execution::headroom::message_limit(
			&crate::bootstrap::execution_headroom(self),
			&run.metadata(),
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn run_media_input_routes(
		&self,
		run: &RunMetadata,
	) -> Result<Vec<Vec<String>>> {
		aidash_application::execution::headroom::media_routes(
			&crate::bootstrap::execution_headroom(self),
			run,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn run_request_headroom(&self, run: &Run) -> Result<usize> {
		aidash_application::execution::headroom::request(
			&crate::bootstrap::execution_headroom(self),
			&run.metadata(),
		)
		.await
		.map_err(Into::into)
	}

	pub async fn deliver_run_messages(&self, run: &Run) -> Result<()> {
		self.deliver_run_message_metadata(&run.metadata()).await
	}

	pub async fn reconcile_run_messages(&self, run: &Run) -> Result<()> {
		aidash_application::federation::run_messages::reconcile(
			&crate::bootstrap::execution_message_scope(self, run),
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn historical_run_message_batch(
		&self,
		run: &Run,
	) -> Result<Vec<(String, Message)>> {
		aidash_application::federation::run_messages::historical_batch(
			&crate::bootstrap::run_message_scope(self, &run.metadata()),
		)
		.await
		.map_err(Into::into)
	}
	pub async fn recover_historical_run_message(
		&self,
		run: &Run,
		key: &str,
		content: &str,
	) -> Result<bool> {
		aidash_application::federation::run_messages::recover_historical(
			&crate::bootstrap::execution_message_scope(self, run),
			key,
			content,
		)
		.await
		.map_err(Into::into)
	}

	/// Workers need reserved database capacity to finish an effect while API
	/// revocations wait for its authority lease. Embedded runners must use this
	/// separate pool too; otherwise waiting API requests can exhaust the pool.
	pub async fn for_workers(&self) -> Result<Self> {
		let store = self.store.isolated_pool().await?;
		Ok(Self {
			registry: Registry::new(store.pool.clone(), &store.deployment)?
				.with_keyless_origins(store.keyless.clone()),
			store,
			..self.clone()
		})
	}
	pub async fn for_recovery(&self) -> Result<Self> {
		let store = self.store.recovery_pool().await?;
		Ok(Self {
			registry: Registry::new(store.pool.clone(), &store.deployment)?
				.with_keyless_origins(store.keyless.clone()),
			store,
			..self.clone()
		})
	}
	pub async fn for_runtime_workers(&self) -> Result<Self> {
		let store = self.store.worker_pool().await?;
		Ok(Self {
			registry: Registry::new(store.pool.clone(), &store.deployment)?
				.with_keyless_origins(store.keyless.clone()),
			store,
			..self.clone()
		})
	}

	/// The Tenant that holds `node`, from the Deployment-wide record of Node names.
	pub(crate) async fn node_tenant_of(
		&self,
		node: &aidash_domain::nodes::NodeName,
	) -> Result<aidash_domain::tenancy::TenantName> {
		let holder = self
			.store
			.pool
			.atomic(&TenancyScope::NoTenant, async |tx| {
				crate::apps::identity::repositories::node_registrations::name_tenant(
					tx,
					node.as_str(),
				)
				.await
			})
			.await?
			.ok_or_else(|| Error::NotFound("node not found".into()))?;
		Ok(aidash_domain::tenancy::TenantName::new(holder)?)
	}
	/// The Tenant that holds the Node at `address`, or `None` when the address
	/// is not a Node of this Deployment.
	pub(crate) async fn tenant_of_node_address(&self, address: &str) -> Result<Option<String>> {
		let Ok(address) = aidash_domain::nodes::NodeAddress::parse(address) else {
			return Ok(None);
		};
		if address.deployment != self.config.deployment {
			return Ok(None);
		}
		self.store
			.pool
			.atomic(&TenancyScope::NoTenant, async |tx| {
				crate::apps::identity::repositories::node_registrations::name_tenant(
					tx,
					address.node.as_str(),
				)
				.await
			})
			.await
	}
	/// The sender's way to `node`, or `Unauthorized` when it has none.
	pub async fn route(&self, sender: &Sender, node: &str) -> Result<Route> {
		self.store
			.pool
			.atomic(&TenancyScope::Tenant(sender.tenant.clone()), async |tx| {
				routes::route_to(tx, &self.config.deployment, sender, node, false).await
			})
			.await?
			.ok_or_else(routes::unregistered)
	}
	/// Every Node the sender addresses.
	pub async fn routes(&self, sender: &Sender) -> Result<Vec<Route>> {
		self.store
			.pool
			.atomic(&TenancyScope::Tenant(sender.tenant.clone()), async |tx| {
				routes::all(tx, &self.config.deployment, sender, false).await
			})
			.await
	}
	pub async fn request<T: DeserializeOwned>(
		&self,
		sender: &Sender,
		node: &str,
		method: Method,
		path: &str,
		body: Option<&Value>,
	) -> Result<T> {
		crate::bootstrap::federation(self, sender)
			.request(node, method.as_str(), path, body)
			.await
			.map_err(Into::into)
	}
	pub(crate) async fn peer_response(
		&self,
		sender: &Sender,
		node: &str,
		method: Method,
		path: &str,
		body: Option<&Value>,
	) -> Result<reqwest::Response> {
		let route = self.route(sender, node).await?;
		super::wire::Wire::of(self)
			.http
			.send(&route, method.as_str(), path, body)
			.await
			.map_err(Into::into)
	}
}

// All worker operations use this single home-node boundary.
#[derive(Clone)]
pub struct Home {
	pub federation: Federation,
	pub run: RunMetadata,
	included_input_seq: i64,
	execution: Option<Run>,
	pub(crate) authority: Option<crate::authorization::execution::WorkerAuthority>,
}

impl Home {
	/// The scope of every local transaction for this Run: its authority's
	/// Tenant, or otherwise the Run's Tenant found by id.
	pub(crate) async fn tenancy(&self) -> Result<TenancyScope> {
		match &self.authority {
			Some(authority) => authority.tenancy().await,
			None => self.federation.store.run_tenancy(self.run.id).await,
		}
	}
	pub async fn human_request(&self, kind: &str, prompt: &str, key: &str) -> Result<HumanRequest> {
		let scope = self.tenancy().await?;
		if self.local() {
			let run = self.execution.as_ref().ok_or(Error::Forbidden)?;
			self.federation
				.store
				.human_request(&scope, run, kind, prompt, key)
				.await
		} else {
			let request: HumanRequest = self
				.command(
					"human_request",
					json!({"run_id":self.run.id,"kind":kind,"prompt":prompt,"key":key}),
				)
				.await?;
			if crate::authorization::peer::admission::run_grant(&self.federation.store, &self.run)
				.await?
				.is_none()
			{
				self.federation
					.store
					.cache_home_human(
						&scope,
						self.execution.as_ref().ok_or(Error::Forbidden)?,
						&request,
						key,
					)
					.await?;
			}
			Ok(request)
		}
	}
	pub async fn human_request_by_id(&self, id: Uuid) -> Result<HumanRequest> {
		let scope = self.tenancy().await?;
		if self.local() {
			self.federation.store.human_request_by_id(&scope, id).await
		} else {
			self.command("human_read", json!({"run_id":self.run.id,"id":id}))
				.await
		}
	}
	/// Answer a Home request on the Run's own behalf: `system` locally, this
	/// node through the delegated route.
	pub async fn close_human_request(&self, id: Uuid, response: Value) -> Result<HumanRequest> {
		if self.local() {
			let scope = self.tenancy().await?;
			self.federation
				.store
				.close_human_request(&scope, id, response)
				.await
		} else {
			self.answer_home_human(id, response).await
		}
	}
	pub(crate) async fn answer_home_human(
		&self,
		id: Uuid,
		response: Value,
	) -> Result<HumanRequest> {
		self.command(
			"human_answer",
			json!({"run_id":self.run.id,"id":id,"response":response}),
		)
		.await
	}

	pub fn new(federation: Federation, run: Run) -> Self {
		Self {
			federation,
			included_input_seq: run.included_input_seq(),
			run: run.metadata(),
			execution: Some(run),
			authority: None,
		}
	}
	pub(crate) fn with_authority(
		mut self,
		authority: Option<crate::authorization::execution::WorkerAuthority>,
	) -> Self {
		self.authority = authority;
		self
	}
	pub async fn discover(&self, search: &Search) -> Result<Discovery> {
		if crate::authorization::peer::admission::run_grant(&self.federation.store, &self.run)
			.await?
			.is_some()
		{
			return Err(Error::Forbidden);
		}
		let Some(authority) = &self.authority else {
			return Err(Error::Forbidden);
		};
		authority.discover(&self.federation, search).await
	}
	pub fn owner(&self) -> String {
		self.run
			.executor_subject(self.federation.config.deployment.as_str())
	}
	pub fn local(&self) -> bool {
		self.run
			.is_local(self.federation.config.deployment.as_str())
	}
	/// The Node at which this remote Run executes, as the Home's callbacks are
	/// sent from it. A callback acts as the Node and names no subject.
	async fn admitted_sender(&self) -> Result<Sender> {
		let scope = self.tenancy().await?;
		let tenant = scope.tenant_name().ok_or(Error::Forbidden)?.clone();
		let node: String = self
			.federation
			.store
			.pool
			.atomic(&scope, async |tx| {
				use reinhardt::db::orm::execution::convert_values;
				use reinhardt::query::{
					Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
				};
				let (sql, values) = Query::select()
					.column(Alias::new("node"))
					.from(Alias::new("authorization_remote_admissions"))
					.and_where(Expr::col("id").eq(Expr::value(self.run.id)))
					.build(PostgresQueryBuilder);
				let row = reinhardt::db::backends::TransactionExecutor::fetch_optional(
					tx,
					&sql,
					convert_values(values),
				)
				.await?
				.ok_or(Error::Forbidden)?;
				Ok(row
					.get("node")
					.map_err(reinhardt::core::exception::Error::from)?)
			})
			.await?;
		Ok(Sender {
			tenant,
			node: Some(aidash_domain::nodes::NodeName::parse(&node)?),
			subject: None,
		})
	}
	pub(crate) async fn command<T: DeserializeOwned>(&self, op: &str, data: Value) -> Result<T> {
		if let Some(grant) =
			crate::authorization::peer::admission::run_grant(&self.federation.store, &self.run)
				.await?
		{
			return crate::authorization::peer::authority_request(
				&self.federation,
				&self.admitted_sender().await?,
				&self.run.home_node,
				"/scoped/execution/commands",
				&json!({"grant_id":grant,"admission_id":self.run.id,"operation":op,"data":data}),
			)
			.await;
		}
		Err(Error::Forbidden)
	}
	pub(crate) async fn optional_command<T: DeserializeOwned>(
		&self,
		op: &str,
		data: Value,
	) -> Result<Option<T>> {
		self.command(op, data).await.map(Some)
	}
	pub async fn snapshot(&self) -> Result<WorkspaceSnapshot> {
		let scope = self.tenancy().await?;
		if crate::authorization::peer::admission::run_grant(&self.federation.store, &self.run)
			.await?
			.is_some()
		{
			return self.command("snapshot", json!({})).await;
		}
		if let Some(authority) = &self.authority {
			authority.snapshot(self.run.workspace_id).await
		} else if self.local() {
			self.federation
				.store
				.snapshot(&scope, self.run.workspace_id)
				.await
		} else {
			let mut snapshot = WorkspaceSnapshot {
				workspace: self.command("snapshot_workspace", json!({})).await?,
				tasks: self.snapshot_collection("tasks").await?,
				artifacts: self.snapshot_collection("artifacts").await?,
				events: self.snapshot_collection("events").await?,
				messages: self.snapshot_collection("messages").await?,
			};
			snapshot
				.tasks
				.sort_by_key(|item| (item.created_at, item.id));
			snapshot
				.artifacts
				.sort_by_key(|item| (item.created_at, item.id));
			snapshot.events.sort_by_key(|item| item.sequence);
			snapshot
				.messages
				.sort_by_key(|item| (item.created_at, item.id));
			Ok(snapshot)
		}
	}
	pub async fn observation(&self, offset: usize, limit: usize) -> Result<Value> {
		if let Some(authority) = &self.authority {
			return authority
				.workspace_observation(self.run.workspace_id, offset, limit)
				.await;
		}
		let snapshot = self.snapshot().await?;
		Ok(crate::context::observation::project(
			&snapshot, offset, limit,
		))
	}
	pub async fn observation_fitted<F>(
		&self,
		offset: usize,
		limit: usize,
		fits: F,
	) -> Result<Option<(usize, Value)>>
	where
		F: FnMut(usize, &Value) -> Result<bool>,
	{
		if let Some(authority) = &self.authority {
			return authority
				.workspace_observation_fitted(self.run.workspace_id, offset, limit, fits)
				.await;
		}
		let snapshot = self.snapshot().await?;
		crate::context::observation::fit_projection(&snapshot, offset, limit, fits)
	}
	pub async fn read_record(&self, kind: &str, id: &str) -> Result<Value> {
		let scope = self.tenancy().await?;
		if let Some(authority) = &self.authority {
			let id = id
				.parse::<Uuid>()
				.map_err(|_| Error::Invalid("invalid workspace record id".into()))?;
			return authority
				.workspace_record(self.run.workspace_id, kind, id)
				.await;
		}
		if self.local() {
			let id = id
				.parse::<Uuid>()
				.map_err(|_| Error::Invalid("invalid workspace record id".into()))?;
			return self
				.federation
				.store
				.workspace_record(&scope, self.run.workspace_id, kind, id)
				.await;
		}
		self.command("workspace_record", json!({"kind":kind,"id":id}))
			.await
	}
	pub async fn read_record_chunk(
		&self,
		kind: &str,
		id: &str,
		offset: usize,
		max_chars: usize,
	) -> Result<Value> {
		let scope = self.tenancy().await?;
		if let Some(authority) = &self.authority {
			let id = id
				.parse::<Uuid>()
				.map_err(|_| Error::Invalid("invalid workspace record id".into()))?;
			let record = authority
				.workspace_record(self.run.workspace_id, kind, id)
				.await?;
			return crate::context::observation::chunk_record(
				record,
				kind,
				&id.to_string(),
				offset,
				max_chars,
			);
		}
		if self.local() {
			let id = id
				.parse::<Uuid>()
				.map_err(|_| Error::Invalid("invalid workspace record id".into()))?;
			let record = self
				.federation
				.store
				.workspace_record(&scope, self.run.workspace_id, kind, id)
				.await?;
			return crate::context::observation::chunk_record(
				record,
				kind,
				&id.to_string(),
				offset,
				max_chars,
			);
		}
		let id = id
			.parse::<Uuid>()
			.map_err(|_| Error::Invalid("invalid workspace record id".into()))?;
		self.command(
			"workspace_record_chunk",
			json!({"kind":kind,"id":id,"offset":offset,"max_chars":max_chars.min(16000)}),
		)
		.await
	}
	pub(crate) async fn child_summary(&self, parent: Uuid) -> Result<ChildTaskSummary> {
		let scope = self.tenancy().await?;
		if let Some(authority) = &self.authority {
			return authority
				.workspace_child_summary(self.run.workspace_id, parent)
				.await;
		}
		if self.local() {
			return self
				.federation
				.store
				.child_task_summary(&scope, self.run.workspace_id, parent)
				.await;
		}
		self.command("workspace_children", json!({"parent_id":parent}))
			.await
	}
	async fn snapshot_collection<T: DeserializeOwned>(&self, collection: &str) -> Result<Vec<T>> {
		let mut items = vec![];
		let mut after: Option<Uuid> = None;
		loop {
			let page: SnapshotPage = self
				.command(
					"snapshot_page",
					json!({"collection":collection,"after":after}),
				)
				.await?;
			items.extend(
				page.items
					.into_iter()
					.map(serde_json::from_value)
					.collect::<std::result::Result<Vec<T>, _>>()?,
			);
			let Some(next) = page.next else {
				return Ok(items);
			};
			if after.is_some_and(|previous| next <= previous) {
				return Err(Error::External(
					"peer snapshot cursor did not advance".into(),
				));
			}
			after = Some(next);
		}
	}
	pub async fn task(&self) -> Result<Task> {
		let scope = self.tenancy().await?;
		if self.authority.is_some() {
			return serde_json::from_value(
				self.read_record("task", &self.run.task_id.to_string())
					.await?,
			)
			.map_err(Into::into);
		}
		if self.local() {
			self.federation.store.task(&scope, self.run.task_id).await
		} else {
			self.command("task", json!({})).await
		}
	}
	pub async fn claim(&self, task: &Task, agent: &Entry) -> Result<Task> {
		let _scope = self.tenancy().await?;
		if task.owner.as_deref() == Some(&self.owner())
			&& task.status != crate::domain::TaskStatus::Open
		{
			return Ok(task.clone());
		}
		if self.local() {
			return Err(Error::Forbidden);
		}
		self.command("claim", json!({"revision":task.revision,"entry":agent}))
			.await
	}
	pub async fn transition(&self, next: TaskStatus) -> Result<Task> {
		let scope = self.tenancy().await?;
		let t = self.task().await?;
		if t.status == next {
			return Ok(t);
		}
		if self.local() {
			self.federation
				.store
				.transition(&scope, t.id, t.revision, &self.owner(), next)
				.await
		} else {
			self.command("transition", json!({"revision":t.revision,"status":next}))
				.await
		}
	}
	pub async fn transition_terminal(&self, next: TaskStatus, through_seq: i64) -> Result<Task> {
		let scope = self.tenancy().await?;
		let task = self.task().await?;
		if self.local() {
			return self
				.federation
				.store
				.transition(&scope, task.id, task.revision, &self.owner(), next)
				.await;
		}
		if let Some(task) = self
			.optional_command(
				"run_message_terminal_transition",
				json!({
					"revision":task.revision,
					"status":next,
					"run_id":self.run.id,
					"keys":[],
					"through_seq":through_seq
				}),
			)
			.await?
		{
			return Ok(task);
		}
		// A preceding home version has no durable fence table, so there is no
		// split fence-consumption operation to race on that peer.
		self.transition(next).await
	}
	pub async fn complete(&self, key: &str, artifact: &ArtifactInput) -> Result<Task> {
		let scope = self.tenancy().await?;
		if self.local() {
			self.federation
				.store
				.complete_from_run(
					&scope,
					self.run.task_id,
					&self.owner(),
					key,
					artifact,
					crate::store::Publication::by(self.run.id, self.authority.is_some()),
					None,
				)
				.await
		} else {
			let through_seq = self.included_input_seq;
			self.command(
				"run_message_complete",
				json!({"run_id":self.run.id,"through_seq":through_seq,"key":key,"artifact":artifact}),
			)
			.await
		}
	}
	pub async fn artifact(&self, key: &str, artifact: &ArtifactInput) -> Result<Artifact> {
		let scope = self.tenancy().await?;
		if self.local() {
			self.federation
				.store
				.publish_artifact_from_run(
					&scope,
					self.run.task_id,
					&self.owner(),
					key,
					artifact,
					crate::store::Publication::by(self.run.id, self.authority.is_some()),
				)
				.await
		} else {
			self.command("artifact", json!({"key":key,"artifact":artifact}))
				.await
		}
	}
	pub async fn assign(
		&self,
		task: Uuid,
		policy: &str,
		reason: &str,
	) -> Result<crate::generation::Assignment> {
		let authority = self.authority.as_ref().ok_or(Error::Forbidden)?;
		authority
			.assign(
				&self.federation,
				self.execution_run()?,
				task,
				policy,
				reason,
			)
			.await
	}
	pub async fn create_task(&self, key: &str, input: &NewTask) -> Result<Task> {
		let scope = self.tenancy().await?;
		if let Some(authority) = &self.authority {
			return authority
				.create_task(&self.federation, self.execution_run()?, key, input)
				.await;
		}
		if self.local() {
			self.federation
				.store
				.create_task(
					&scope,
					self.run.workspace_id,
					input,
					&self.owner(),
					Some(key),
				)
				.await
		} else {
			self.command("create_task", json!({"key":key,"task":input}))
				.await
		}
	}
	pub async fn delegate(
		&self,
		task_id: Uuid,
		node: &str,
		agent: &EntityRef,
	) -> Result<Delegation> {
		let key = format!(
			"run:{}:task:{}:node:{}:agent:{}@{}",
			self.run.id, task_id, node, agent.id, agent.version
		);
		self.delegate_with_key(&key, task_id, node, agent).await
	}
	pub async fn delegate_with_key(
		&self,
		key: &str,
		task_id: Uuid,
		node: &str,
		agent: &EntityRef,
	) -> Result<Delegation> {
		let _scope = self.tenancy().await?;
		if let Some(authority) = &self.authority {
			return authority
				.delegate(
					&self.federation,
					self.execution_run()?,
					task_id,
					node,
					agent,
				)
				.await;
		}
		if self.local() {
			return Err(Error::Forbidden);
		}
		self.command(
			"delegate",
			json!({"key":key,"task_id":task_id,"node_id":node,"agent":agent}),
		)
		.await
	}
	pub async fn message(&self, key: &str, content: &str) -> Result<()> {
		let scope = self.tenancy().await?;
		if self.authority.is_some() || self.local() {
			let run = match self.authority {
				Some(_) => self.execution_run()?.metadata(),
				None => self.run.clone(),
			};
			self.federation
				.store
				.message_from_run(
					&scope,
					&run,
					&self.owner(),
					content,
					key,
					self.authority.is_some(),
				)
				.await
		} else {
			self.command::<Value>("message", json!({"key":key,"content":content}))
				.await?;
			Ok(())
		}
	}
	pub async fn response_message(
		&self,
		worker: Uuid,
		included_input_seq: i64,
		key: &str,
		content: &str,
	) -> Result<()> {
		let scope = self.tenancy().await?;
		if self.authority.is_some() || self.local() {
			self.federation
				.store
				.response_message_in_run(
					&scope,
					RunResponseMessage {
						run: self.execution_run()?,
						worker,
						included_input_seq,
						sender: &self.owner(),
						content,
						key,
						track_output: self.authority.is_some(),
					},
				)
				.await
		} else {
			self.federation
				.store
				.with_run_response_fence(
					&scope, self.run.id,
					worker,
					included_input_seq,
					self.command::<Value>(
						"run_message_output",
						json!({"run_id":self.run.id,"included_input_seq":included_input_seq,"key":key,"content":content}),
					),
				)
				.await?;
			Ok(())
		}
	}
	pub async fn human_message(&self, key: &str, content: &str) -> Result<()> {
		let scope = self.tenancy().await?;
		if self.local() {
			self.federation
				.store
				.message(&scope, self.run.workspace_id, "human", content, Some(key))
				.await
		} else {
			self.command::<Value>("human_message", json!({"key":key,"content":content}))
				.await?;
			Ok(())
		}
	}
	pub async fn reserve_run_message(&self, key: &str, content: &str) -> Result<bool> {
		aidash_application::federation::run_messages::reserve(
			&crate::bootstrap::run_message_scope(&self.federation, &self.run),
			key,
			content,
		)
		.await
		.map_err(Into::into)
	}
	pub async fn commit_run_message(&self, key: &str, content: &str) -> Result<()> {
		aidash_application::federation::run_messages::commit(
			&crate::bootstrap::run_message_scope(&self.federation, &self.run),
			key,
			content,
		)
		.await
		.map_err(Into::into)
	}
	pub async fn release_run_messages(&self, keys: &[String]) -> Result<()> {
		aidash_application::federation::run_messages::release(
			&crate::bootstrap::run_message_scope(&self.federation, &self.run),
			keys,
		)
		.await
		.map_err(Into::into)
	}
	pub async fn acknowledge_run_messages(&self, keys: &[String]) -> Result<()> {
		aidash_application::federation::run_messages::acknowledge(
			&crate::bootstrap::run_message_scope(&self.federation, &self.run),
			keys,
		)
		.await
		.map_err(Into::into)
	}
	pub async fn human_message_record(&self, key: &str, content: &str) -> Result<Message> {
		aidash_application::federation::run_messages::record(
			&crate::bootstrap::run_message_scope(&self.federation, &self.run),
			key,
			content,
		)
		.await
		.map_err(Into::into)
	}
	pub async fn historical_run_messages(&self) -> Result<Vec<Message>> {
		aidash_application::federation::run_messages::history(&crate::bootstrap::run_message_scope(
			&self.federation,
			&self.run,
		))
		.await
		.map_err(Into::into)
	}
	pub async fn report(&self, key: &str, kind: &str, data: Value) -> Result<()> {
		if self.local() {
			return Ok(());
		}
		self.command::<Value>("event", json!({"key":key,"kind":kind,"data":data}))
			.await?;
		Ok(())
	}
}

#[cfg(test)]
#[path = "../tests/services_runtime_review_tests.rs"]
mod review_tests;

#[async_trait::async_trait]
impl Injectable for Federation {
	async fn inject(ctx: &InjectionContext) -> DiResult<Self> {
		ctx.get_singleton::<Self>()
			.map(|value| (*value).clone())
			.ok_or_else(|| DiError::NotFound("Aidash runtime".into()))
	}
}

pub use crate::apps::federation::remote::serializers::runtime::{
	Delegation, DiscoveredAgent, Discovery, Offer,
};

impl Federation {
	pub(crate) async fn transition_terminal_metadata(
		&self,
		run: &RunMetadata,
		target: TaskStatus,
		authority: Option<crate::authorization::execution::WorkerAuthority>,
	) -> Result<Task> {
		let scope = self.store.run_tenancy(run.id).await?;
		let through_seq = self.store.run_input_high_watermark(&scope, run.id).await?;
		let home = Home::for_delivery(self.clone(), run.clone()).with_authority(authority);
		home.transition_terminal(target, through_seq).await
	}
}

impl Federation {
	pub(crate) async fn deliver_run_message_metadata(&self, run: &RunMetadata) -> Result<()> {
		aidash_application::federation::run_messages::deliver(&crate::bootstrap::run_message_scope(
			self, run,
		))
		.await
		.map_err(Into::into)
	}
}

impl Home {
	pub(crate) fn for_delivery(federation: Federation, run: RunMetadata) -> Self {
		Self {
			included_input_seq: run.observed_input_seq,
			federation,
			run,
			execution: None,
			authority: None,
		}
	}
}

impl Home {
	fn execution_run(&self) -> Result<&Run> {
		self.execution
			.as_ref()
			.ok_or_else(|| Error::Invalid("delivery has no executable continuation".into()))
	}
}
