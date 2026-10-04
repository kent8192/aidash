use crate::apps::federation::peer::services::records::PeerRecords;
use crate::apps::federation::remote::models::Delegation as DelegationRecord;
use crate::{
	Error, Result,
	config::{Config, PROTOCOL_VERSION, peer_secret, validate_endpoint, validate_node_id},
	domain::*,
	registry::{EntityRef, Entry, Registry, Search},
	store::{RunResponseMessage, Store},
};
use reinhardt::DiError;
use reinhardt::DiResult;
use reinhardt::Injectable;
use reinhardt::InjectionContext;
use reinhardt::db::backends::TransactionExecutor;
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
			registry: Registry::new(store.pool.clone(), &store.node_id)?,
			store,
			..self.clone()
		})
	}
	pub async fn for_recovery(&self) -> Result<Self> {
		let store = self.store.recovery_pool().await?;
		Ok(Self {
			registry: Registry::new(store.pool.clone(), &store.node_id)?,
			store,
			..self.clone()
		})
	}
	pub async fn for_runtime_workers(&self) -> Result<Self> {
		let store = self.store.worker_pool().await?;
		Ok(Self {
			registry: Registry::new(store.pool.clone(), &store.node_id)?,
			store,
			..self.clone()
		})
	}

	pub async fn peers(&self) -> Result<Vec<Peer>> {
		let lease = self.store.orm_connection()?;
		PeerRecords::new(lease.handle(), &self.config.node_id)
			.list()
			.await
	}
	pub async fn peer(&self, node: &str) -> Result<Peer> {
		let lease = self.store.orm_connection()?;
		PeerRecords::new(lease.handle(), &self.config.node_id)
			.enabled(node)
			.await
	}

	pub async fn register_peer(&self, peer: Peer) -> Result<Peer> {
		validate_node_id(&peer.node_id)?;
		validate_endpoint(&peer.endpoint)?;
		if peer.node_id == self.config.node_id || peer.protocol_version != PROTOCOL_VERSION {
			return Err(Error::Invalid(
				"peer must be another node with protocol_version 0.1".into(),
			));
		}
		let lease = self.store.orm_connection()?;
		let records = PeerRecords::new(lease.handle(), &self.config.node_id);
		if !peer.enabled {
			return records.disable(&peer.node_id).await;
		}

		let credential = peer_secret(&peer.credential_env)?;
		let identity = crate::bootstrap::peer_transport(self)
			.identity(&peer)
			.await?;
		if identity["id"] != peer.node_id || identity["protocol_version"] != PROTOCOL_VERSION {
			return Err(Error::Invalid(
				"peer identity or protocol does not match".into(),
			));
		}
		records.register(peer, &credential).await
	}

	pub async fn authenticate_peer(&self, node: &str, supplied: &str) -> Result<()> {
		let peer = self.peer(node).await?;
		let credential = peer_secret(&peer.credential_env)?;
		if !crate::config::same_secret(supplied, &credential) {
			return Err(Error::Unauthorized);
		}
		// Also reject ambiguous existing configurations and environment rotation.
		for other in self
			.peers()
			.await?
			.into_iter()
			.filter(|p| p.enabled && p.node_id != node)
		{
			if peer_secret(&other.credential_env).is_ok_and(|key| key == credential) {
				return Err(Error::Unauthorized);
			}
		}
		Ok(())
	}
	pub async fn request<T: DeserializeOwned>(
		&self,
		node: &str,
		method: Method,
		path: &str,
		body: Option<&Value>,
	) -> Result<T> {
		crate::bootstrap::federation(self)
			.request(node, method.as_str(), path, body)
			.await
			.map_err(Into::into)
	}
	pub(crate) async fn peer_response(
		&self,
		node: &str,
		method: Method,
		path: &str,
		body: Option<&Value>,
	) -> Result<reqwest::Response> {
		let peer = self.peer(node).await?;
		crate::bootstrap::peer_transport(self)
			.send(&peer, method.as_str(), path, body)
			.await
			.map_err(Into::into)
	}

	pub async fn discover(&self, search: &Search) -> Result<Discovery> {
		crate::bootstrap::federation(self)
			.discover(search)
			.await
			.map_err(Into::into)
	}
	pub async fn delegate(
		&self,
		task_id: Uuid,
		node: &str,
		agent: &EntityRef,
	) -> Result<Delegation> {
		crate::bootstrap::federation(self)
			.delegate(task_id, node, agent)
			.await
			.map_err(Into::into)
	}
	pub(crate) async fn delegate_in(
		&self,
		tx: &mut dyn TransactionExecutor,
		task: &Task,
		node: &str,
		agent: &EntityRef,
	) -> Result<(Task, Delegation)> {
		DelegationRecord::reserve(tx, &self.config.node_id, task, node, agent).await
	}
	pub async fn deliver(&self, delegation: &Delegation) -> Result<()> {
		crate::bootstrap::federation(self)
			.deliver(delegation)
			.await
			.map_err(Into::into)
	}
	pub async fn retry_deliveries(&self) -> Result<()> {
		crate::bootstrap::federation(self)
			.retry_deliveries()
			.await
			.map_err(Into::into)
	}
	pub async fn authorize_task(&self, node: &str, task_id: Uuid, agent: &EntityRef) -> Result<()> {
		let lease = self.store.orm_connection()?;
		let allowed =
			DelegationRecord::authorized(&mut lease.handle(), task_id, node, agent).await?;
		if !allowed {
			return Err(Error::Unauthorized);
		}
		Ok(())
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
		if let Some(authority) = &self.authority {
			authority.discover(&self.federation, search).await
		} else {
			self.federation.discover(search).await
		}
	}
	pub fn owner(&self) -> String {
		qualified_agent(
			&self.federation.config.node_id,
			&self.run.agent_id,
			&self.run.agent_version,
		)
	}
	pub fn local(&self) -> bool {
		self.run.home_node == self.federation.config.node_id
	}
	pub(crate) async fn command<T: DeserializeOwned>(&self, op: &str, data: Value) -> Result<T> {
		if let Some(grant) =
			crate::authorization::peer::admission::run_grant(&self.federation.store, &self.run)
				.await?
		{
			return crate::authorization::peer::authority_request(
				&self.federation,
				&self.run.home_node,
				"/scoped/execution/commands",
				&json!({"grant_id":grant,"admission_id":self.run.id,"operation":op,"data":data}),
			)
			.await;
		}
		self.federation.request(&self.run.home_node,Method::POST,"/workspace",Some(&json!({"task_id":self.run.task_id,"agent":{"id":self.run.agent_id,"version":self.run.agent_version},"operation":op,"data":data}))).await
	}
	pub(crate) async fn optional_command<T: DeserializeOwned>(
		&self,
		op: &str,
		data: Value,
	) -> Result<Option<T>> {
		if crate::authorization::peer::admission::run_grant(&self.federation.store, &self.run)
			.await?
			.is_some()
		{
			return self.command(op, data).await.map(Some);
		}
		let command = json!({"task_id":self.run.task_id,"agent":{"id":self.run.agent_id,"version":self.run.agent_version},"operation":op,"data":data});
		let response = self
			.federation
			.peer_response(
				&self.run.home_node,
				Method::POST,
				"/workspace",
				Some(&command),
			)
			.await?;
		if response.status() == reqwest::StatusCode::BAD_REQUEST {
			let body: Value = crate::response::json(response, 4096).await?;
			if body["error"] == "unknown federation operation" {
				return Ok(None);
			}
			return Err(Error::External(format!(
				"peer {} rejected {op}: {}",
				self.run.home_node, body["error"]
			)));
		}
		if response.status() == reqwest::StatusCode::CONFLICT {
			return Err(Error::Conflict("remote task state changed".into()));
		}
		let response = response.error_for_status()?;
		Ok(Some(crate::response::json(response, 4_194_304).await?))
	}
	pub async fn snapshot(&self) -> Result<WorkspaceSnapshot> {
		if crate::authorization::peer::admission::run_grant(&self.federation.store, &self.run)
			.await?
			.is_some()
		{
			return self.command("snapshot", json!({})).await;
		}
		if let Some(authority) = &self.authority {
			authority.snapshot(self.run.workspace_id).await
		} else if self.local() {
			self.federation.store.snapshot(self.run.workspace_id).await
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
				.workspace_record(self.run.workspace_id, kind, id)
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
		if let Some(authority) = &self.authority {
			return authority
				.workspace_child_summary(self.run.workspace_id, parent)
				.await;
		}
		if self.local() {
			return self
				.federation
				.store
				.child_task_summary(self.run.workspace_id, parent)
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
		if self.authority.is_some() {
			return serde_json::from_value(
				self.read_record("task", &self.run.task_id.to_string())
					.await?,
			)
			.map_err(Into::into);
		}
		if self.local() {
			self.federation.store.task(self.run.task_id).await
		} else {
			self.command("task", json!({})).await
		}
	}
	pub async fn claim(&self, task: &Task, agent: &Entry) -> Result<Task> {
		if task.owner.as_deref() == Some(&self.owner())
			&& task.status != crate::domain::TaskStatus::Open
		{
			return Ok(task.clone());
		}
		if self.local() {
			self.federation
				.store
				.claim(task.id, task.revision, &self.owner(), agent)
				.await
		} else {
			self.command("claim", json!({"revision":task.revision,"entry":agent}))
				.await
		}
	}
	pub async fn transition(&self, next: TaskStatus) -> Result<Task> {
		let t = self.task().await?;
		if t.status == next {
			return Ok(t);
		}
		if self.local() {
			self.federation
				.store
				.transition(t.id, t.revision, &self.owner(), next)
				.await
		} else {
			self.command("transition", json!({"revision":t.revision,"status":next}))
				.await
		}
	}
	pub async fn transition_terminal(&self, next: TaskStatus, through_seq: i64) -> Result<Task> {
		let task = self.task().await?;
		if self.local() {
			return self
				.federation
				.store
				.transition(task.id, task.revision, &self.owner(), next)
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
		if self.local() {
			self.federation
				.store
				.complete_from_run(
					self.run.task_id,
					&self.owner(),
					key,
					artifact,
					self.authority.as_ref().map(|_| self.run.id),
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
		if self.local() {
			self.federation
				.store
				.publish_artifact_from_run(
					self.run.task_id,
					&self.owner(),
					key,
					artifact,
					self.authority.as_ref().map(|_| self.run.id),
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
		if let Some(authority) = &self.authority {
			return authority
				.create_task(&self.federation, self.execution_run()?, key, input)
				.await;
		}
		if self.local() {
			self.federation
				.store
				.create_task(self.run.workspace_id, input, &self.owner(), Some(key))
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
			let t = self.federation.store.task(task_id).await?;
			if t.workspace_id != self.run.workspace_id {
				return Err(Error::Unauthorized);
			}
			self.federation.delegate(task_id, node, agent).await
		} else {
			self.command(
				"delegate",
				json!({"key":key,"task_id":task_id,"node_id":node,"agent":agent}),
			)
			.await
		}
	}
	pub async fn message(&self, key: &str, content: &str) -> Result<()> {
		if self.authority.is_some() {
			self.federation
				.store
				.message_from_run(self.execution_run()?, &self.owner(), content, key)
				.await
		} else if self.local() {
			self.federation
				.store
				.message(self.run.workspace_id, &self.owner(), content, Some(key))
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
		if self.authority.is_some() || self.local() {
			self.federation
				.store
				.response_message_in_run(RunResponseMessage {
					run: self.execution_run()?,
					worker,
					included_input_seq,
					sender: &self.owner(),
					content,
					key,
					track_output: self.authority.is_some(),
				})
				.await
		} else {
			self.federation
				.store
				.with_run_response_fence(
					self.run.id,
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
		if self.local() {
			self.federation
				.store
				.message(self.run.workspace_id, "human", content, Some(key))
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
fn map_workspace_chunk_bad_request(
	path: &str,
	request: Option<&Value>,
	error: &Value,
) -> Option<Error> {
	aidash_application::federation::map_workspace_chunk_bad_request(path, request, error)
		.map(Into::into)
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
	Delegation, DiscoveredAgent, Discovery, Offer, Peer,
};

impl Federation {
	pub(crate) async fn transition_terminal_metadata(
		&self,
		run: &RunMetadata,
		target: TaskStatus,
		authority: Option<crate::authorization::execution::WorkerAuthority>,
	) -> Result<Task> {
		let through_seq = self.store.run_input_high_watermark(run.id).await?;
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
