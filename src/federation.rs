use crate::{
	Error, Result,
	config::{Config, PROTOCOL_VERSION, peer_secret, validate_endpoint, validate_node_id},
	domain::*,
	registry::{AgentConfig, EntityRef, Entry, Registry, Search},
	store::Store,
};
use futures_util::{StreamExt, stream};
use reqwest::Method;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Peer {
	pub node_id: String,
	pub endpoint: String,
	pub credential_env: String,
	pub protocol_version: String,
	pub enabled: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DiscoveredAgent {
	pub node_id: String,
	pub entity: Entry,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Discovery {
	pub agents: Vec<DiscoveredAgent>,
	pub errors: Vec<crate::api_schema::PeerError>,
}
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Delegation {
	pub task_id: Uuid,
	pub node_id: String,
	pub agent_id: String,
	pub agent_version: String,
	pub delivered: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Offer {
	pub task: Task,
	pub agent: EntityRef,
}

#[derive(Clone)]
pub struct Federation {
	pub store: Store,
	pub registry: Registry,
	pub config: Config,
	pub client: reqwest::Client,
	pub notify: std::sync::Arc<tokio::sync::Notify>,
}
impl Federation {
	/// Workers need reserved database capacity to finish an effect while API
	/// revocations wait for its authority lease. Embedded runners must use this
	/// separate pool too; otherwise waiting API requests can exhaust the pool.
	pub async fn for_workers(&self) -> Result<Self> {
		let store = self.store.isolated_pool().await?;
		Ok(Self {
			registry: Registry::new(store.pool.clone(), &store.node_id),
			store,
			..self.clone()
		})
	}
	pub async fn for_recovery(&self) -> Result<Self> {
		let store = self.store.recovery_pool().await?;
		Ok(Self {
			registry: Registry::new(store.pool.clone(), &store.node_id),
			store,
			..self.clone()
		})
	}
	pub async fn for_runtime_workers(&self) -> Result<Self> {
		let store = self.store.worker_pool().await?;
		Ok(Self {
			registry: Registry::new(store.pool.clone(), &store.node_id),
			store,
			..self.clone()
		})
	}

	pub async fn peers(&self) -> Result<Vec<Peer>> {
		Ok(sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
				))
				.from(sea_orm::sea_query::Alias::new("peers"))
				.order_by_expr(
					sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
						sea_orm::sea_query::Alias::new("node_id"),
					)),
					sea_orm::sea_query::Order::Asc,
				)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.fetch_all(&self.store.pool)
		.await?)
	}
	pub async fn peer(&self, node: &str) -> Result<Peer> {
		sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
				))
				.from(sea_orm::sea_query::Alias::new("peers"))
				.and_where(sea_orm::sea_query::Expr::cust("node_id = $1 AND enabled"))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(node)
		.fetch_optional(&self.store.pool)
		.await?
		.ok_or_else(|| Error::Unauthorized)
	}
	pub async fn register_peer(&self, peer: Peer) -> Result<Peer> {
		validate_node_id(&peer.node_id)?;
		validate_endpoint(&peer.endpoint)?;
		if peer.node_id == self.config.node_id || peer.protocol_version != PROTOCOL_VERSION {
			return Err(Error::Invalid(
				"peer must be another node with protocol_version 0.1".into(),
			));
		}
		if !peer.enabled {
			let mut tx = self.store.pool.begin().await?;
			let existing: Peer = sqlx::query_as(
				&sea_orm::sea_query::Query::update()
					.table(sea_orm::sea_query::Alias::new("peers"))
					.value(
						sea_orm::sea_query::Alias::new("enabled"),
						sea_orm::sea_query::Expr::cust("FALSE"),
					)
					.and_where(sea_orm::sea_query::Expr::cust("node_id = $1"))
					.returning_all()
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(&peer.node_id)
			.fetch_optional(&mut *tx)
			.await?
			.ok_or_else(|| Error::NotFound("peer".into()))?;
			self.store
				.event(
					&mut tx,
					None,
					"peer.registered",
					json!({"node_id":existing.node_id,"endpoint":existing.endpoint,"enabled":false}),
				)
				.await?;
			tx.commit().await?;
			return Ok(existing);
		}
		let credential = peer_secret(&peer.credential_env)?;
		let response = self
			.client
			.get(format!(
				"{}/.well-known/aidash",
				peer.endpoint.trim_end_matches('/')
			))
			.timeout(Duration::from_secs(5))
			.send()
			.await?
			.error_for_status()?;
		let identity: Value = crate::response::json(response, 1_048_576).await?;
		if identity["id"] != peer.node_id || identity["protocol_version"] != PROTOCOL_VERSION {
			return Err(Error::Invalid(
				"peer identity or protocol does not match".into(),
			));
		}
		let mut tx = self.store.pool.begin().await?;
		sqlx::query(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust(
					"PG_ADVISORY_XACT_LOCK(71003203)",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?;
		let peers: Vec<Peer> = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
				))
				.from(sea_orm::sea_query::Alias::new("peers"))
				.and_where(sea_orm::sea_query::Expr::cust("enabled AND node_id <> $1"))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(&peer.node_id)
		.fetch_all(&mut *tx)
		.await?;
		for other in peers {
			if peer_secret(&other.credential_env)? == credential {
				return Err(Error::Invalid(
					"enabled peers must use distinct credentials for each node identity".into(),
				));
			}
		}
		sqlx::query(
			&sea_orm::sea_query::Query::insert()
				.into_table(sea_orm::sea_query::Alias::new("peers"))
				.columns([
					sea_orm::sea_query::Alias::new("node_id"),
					sea_orm::sea_query::Alias::new("endpoint"),
					sea_orm::sea_query::Alias::new("credential_env"),
					sea_orm::sea_query::Alias::new("protocol_version"),
					sea_orm::sea_query::Alias::new("enabled"),
				])
				.values_panic([
					sea_orm::sea_query::Expr::cust("$1"),
					sea_orm::sea_query::Expr::cust("$2"),
					sea_orm::sea_query::Expr::cust("$3"),
					sea_orm::sea_query::Expr::cust("$4"),
					sea_orm::sea_query::Expr::cust("$5"),
				])
				.on_conflict(
					sea_orm::sea_query::OnConflict::columns([sea_orm::sea_query::Alias::new(
						"node_id",
					)])
					.value(
						sea_orm::sea_query::Alias::new("endpoint"),
						sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col((
							sea_orm::sea_query::Alias::new("excluded"),
							sea_orm::sea_query::Alias::new("endpoint"),
						))),
					)
					.value(
						sea_orm::sea_query::Alias::new("credential_env"),
						sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col((
							sea_orm::sea_query::Alias::new("excluded"),
							sea_orm::sea_query::Alias::new("credential_env"),
						))),
					)
					.value(
						sea_orm::sea_query::Alias::new("protocol_version"),
						sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col((
							sea_orm::sea_query::Alias::new("excluded"),
							sea_orm::sea_query::Alias::new("protocol_version"),
						))),
					)
					.value(
						sea_orm::sea_query::Alias::new("enabled"),
						sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col((
							sea_orm::sea_query::Alias::new("excluded"),
							sea_orm::sea_query::Alias::new("enabled"),
						))),
					)
					.to_owned(),
				)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(&peer.node_id)
		.bind(&peer.endpoint)
		.bind(&peer.credential_env)
		.bind(&peer.protocol_version)
		.bind(peer.enabled)
		.execute(&mut *tx)
		.await?;
		self.store
			.event(
				&mut tx,
				None,
				"peer.registered",
				json!({"node_id":peer.node_id,"endpoint":peer.endpoint,"enabled":peer.enabled}),
			)
			.await?;
		tx.commit().await?;
		Ok(peer)
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
		let response = self.peer_response(node, method, path, body).await?;
		let status = response.status();
		if !status.is_success() {
			return Err(
				if status == reqwest::StatusCode::SERVICE_UNAVAILABLE
					&& response
						.headers()
						.get("x-aidash-transaction-pending")
						.is_some_and(|value| value == "1")
				{
					Error::TransactionPending
				} else if status == reqwest::StatusCode::CONFLICT {
					Error::Conflict("remote task state changed".into())
				} else if status == reqwest::StatusCode::BAD_REQUEST {
					let error_body = response.json::<Value>().await.unwrap_or(Value::Null);
					map_workspace_chunk_bad_request(path, body, &error_body).unwrap_or_else(|| {
						Error::External(format!("peer {node} returned {status}"))
					})
				} else {
					Error::External(format!("peer {node} returned {status}"))
				},
			);
		}
		crate::response::json(response, 4_194_304).await
	}
	pub(crate) async fn peer_response(
		&self,
		node: &str,
		method: Method,
		path: &str,
		body: Option<&Value>,
	) -> Result<reqwest::Response> {
		let peer = self.peer(node).await?;
		let mut request = self
			.client
			.request(
				method,
				format!(
					"{}/federation/v0.1{}",
					peer.endpoint.trim_end_matches('/'),
					path
				),
			)
			.timeout(Duration::from_secs(10))
			.bearer_auth(peer_secret(&peer.credential_env)?)
			.header("x-aidash-node", &self.config.node_id)
			.header("x-aidash-protocol", PROTOCOL_VERSION);
		if let Some(body) = body {
			request = request.json(body);
		}
		Ok(request.send().await?)
	}
	async fn discovery_entries(&self, node: &str, search: &Search) -> Result<Vec<Entry>> {
		let mut offset = 0;
		let mut entries = vec![];
		loop {
			let page = if node == self.config.node_id {
				self.registry.legacy_agents(search, offset).await?
			} else {
				self.request::<crate::registry::AgentPage>(
					node,
					Method::POST,
					&format!("/discover?offset={offset}"),
					Some(&json!(search)),
				)
				.await?
			};
			entries.extend(page.entries);
			match page.next_offset {
				Some(next) if next > offset => offset = next,
				Some(_) => return Err(Error::External("discovery cursor did not advance".into())),
				None => return Ok(entries),
			}
		}
	}
	pub async fn discover(&self, search: &Search) -> Result<Discovery> {
		let mut query = search.clone();
		query.kind = Some("agent".into());
		let mut result = Discovery {
			agents: self
				.discovery_entries(&self.config.node_id, &query)
				.await?
				.into_iter()
				.map(|entity| DiscoveredAgent {
					node_id: self.config.node_id.clone(),
					entity,
				})
				.collect(),
			errors: vec![],
		};
		let peers = self.peers().await?.into_iter().filter(|p| p.enabled);
		let mut responses = stream::iter(peers.map(|peer| {
			let query = &query;
			async move {
				let response = self.discovery_entries(&peer.node_id, query).await;
				(peer, response)
			}
		}))
		.buffer_unordered(8);
		while let Some((peer, response)) = responses.next().await {
			match response {
				Ok(entries) => {
					result
						.agents
						.extend(
							entries
								.into_iter()
								.filter(|e| query.matches(e))
								.map(|entity| DiscoveredAgent {
									node_id: peer.node_id.clone(),
									entity,
								}),
						)
				}
				Err(e) => result.errors.push(crate::api_schema::PeerError {
					node_id: peer.node_id,
					error: e.to_string(),
				}),
			}
		}
		Ok(result)
	}
	pub async fn delegate(
		&self,
		task_id: Uuid,
		node: &str,
		agent: &EntityRef,
	) -> Result<Delegation> {
		let task = self.store.task(task_id).await?;
		self.store
			.require_legacy_execution(task.workspace_id)
			.await?;
		if task.status != "OPEN"
			&& task.owner.as_deref() != Some(&qualified_agent(node, &agent.id, &agent.version))
		{
			return Err(Error::Conflict("task is already assigned".into()));
		}
		if node == self.config.node_id {
			self.store
				.require_legacy_agent(&agent.id, &agent.version)
				.await?;
			let entry = self.registry.get(&agent.id, &agent.version).await?;
			let _: AgentConfig = serde_json::from_value(entry.config.clone())
				.map_err(|_| Error::Invalid("executor must be an agent".into()))?;
			let q: Search = serde_json::from_value(task.requirements.clone())?;
			if entry.kind != "agent" || !q.matches(&entry) {
				return Err(Error::Invalid(
					"agent does not satisfy task requirements".into(),
				));
			}
		} else {
			let mut search: Search = serde_json::from_value(task.requirements.clone())?;
			search.kind = Some("agent".into());
			let entry: Entry = self
				.request(
					node,
					Method::GET,
					&format!("/discover/{}/{}", agent.id, agent.version),
					None,
				)
				.await?;
			if entry.id != agent.id || entry.version != agent.version || !search.matches(&entry) {
				return Err(Error::Invalid(
					"remote agent is missing or does not satisfy task requirements".into(),
				));
			}
		}
		let mut tx = self.store.pool.begin().await?;
		let mut d = self.delegate_in(&mut tx, &task, node, agent).await?;
		tx.commit().await?;
		match self.deliver(&d).await {
			Ok(()) => d.delivered = true,
			Err(e) => tracing::warn!(error=%e,task_id=%task_id,"delegation queued for retry"),
		}
		Ok(d)
	}
	pub(crate) async fn delegate_in(
		&self,
		tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
		task: &Task,
		node: &str,
		agent: &EntityRef,
	) -> Result<Delegation> {
		use sea_orm::sea_query::{Alias, Asterisk, Expr, LockType, PostgresQueryBuilder, Query};
		let task_id = task.id;
		let current: Task = sqlx::query_as(
			&Query::select()
				.column(Asterisk)
				.from(Alias::new("tasks"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.bind(task_id)
		.fetch_one(&mut **tx)
		.await?;
		let owner = qualified_agent(node, &agent.id, &agent.version);
		if current.revision != task.revision
			|| (current.owner.is_some() && current.owner.as_deref() != Some(&owner))
			|| (current.status != "OPEN" && current.owner.as_deref() != Some(&owner))
		{
			return Err(Error::Conflict(
				"task changed or is already assigned".into(),
			));
		}
		let inserted = sqlx::query(
			&sea_orm::sea_query::Query::insert()
				.into_table(sea_orm::sea_query::Alias::new("delegations"))
				.columns([
					sea_orm::sea_query::Alias::new("task_id"),
					sea_orm::sea_query::Alias::new("node_id"),
					sea_orm::sea_query::Alias::new("agent_id"),
					sea_orm::sea_query::Alias::new("agent_version"),
				])
				.values_panic([
					sea_orm::sea_query::Expr::cust("$1"),
					sea_orm::sea_query::Expr::cust("$2"),
					sea_orm::sea_query::Expr::cust("$3"),
					sea_orm::sea_query::Expr::cust("$4"),
				])
				.on_conflict(
					sea_orm::sea_query::OnConflict::new()
						.do_nothing()
						.to_owned(),
				)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(task_id)
		.bind(node)
		.bind(&agent.id)
		.bind(&agent.version)
		.execute(&mut **tx)
		.await?;
		if inserted.rows_affected() > 0 && current.status == "OPEN" {
			// The delegation record reserves the claimant while the task stays
			// OPEN for dependency waiting. Bump its revision under this lock so
			// a concurrent claim cannot commit against a pre-delegation snapshot.
			sqlx::query(
				&Query::update()
					.table(Alias::new("tasks"))
					.value(
						Alias::new("revision"),
						Expr::col(Alias::new("revision")).add(1),
					)
					.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
					.to_string(PostgresQueryBuilder),
			)
			.bind(task_id)
			.execute(&mut **tx)
			.await?;
		}
		let d: Delegation = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("task_id")),
				))
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("node_id")),
				))
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("agent_id")),
				))
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("agent_version")),
				))
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("delivered")),
				))
				.from(sea_orm::sea_query::Alias::new("delegations"))
				.and_where(sea_orm::sea_query::Expr::cust("task_id = $1"))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(task_id)
		.fetch_one(&mut **tx)
		.await?;
		if d.node_id != node || d.agent_id != agent.id || d.agent_version != agent.version {
			return Err(Error::Conflict(
				"task already delegated to a different agent".into(),
			));
		}
		if inserted.rows_affected() > 0 {
			self.store
				.event(tx, Some(task.workspace_id), "task.delegated", json!(d))
				.await?;
		}
		Ok(d)
	}
	pub async fn deliver(&self, d: &Delegation) -> Result<()> {
		if d.delivered {
			return Ok(());
		}
		let task = self.store.task(d.task_id).await?;
		let agent = EntityRef {
			id: d.agent_id.clone(),
			version: d.agent_version.clone(),
		};
		if d.node_id == self.config.node_id {
			self.store
				.accept_run(&task, &self.config.node_id, &agent.id, &agent.version)
				.await?;
		} else {
			self.request::<Run>(
				&d.node_id,
				Method::POST,
				"/offers",
				Some(&json!(Offer { task, agent })),
			)
			.await?;
		}
		sqlx::query(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("delegations"))
				.value(
					sea_orm::sea_query::Alias::new("delivered"),
					sea_orm::sea_query::Expr::cust("TRUE"),
				)
				.and_where(sea_orm::sea_query::Expr::cust("task_id = $1"))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(d.task_id)
		.execute(&self.store.pool)
		.await?;
		Ok(())
	}
	pub async fn retry_deliveries(&self) -> Result<()> {
		let _visibility = crate::transactions::gate::ReadLease::begin(&self.store).await?;
		let pending:Vec<Delegation>=sqlx::query_as(&sea_orm::sea_query::Query::update().table(sea_orm::sea_query::Alias::new("delegations")).value(sea_orm::sea_query::Alias::new("next_attempt_at"), sea_orm::sea_query::Expr::cust("CURRENT_TIMESTAMP + INTERVAL '5 SECONDS'")).and_where(sea_orm::sea_query::Expr::cust("task_id IN (SELECT task_id FROM delegations WHERE NOT delivered AND next_attempt_at <= CURRENT_TIMESTAMP ORDER BY next_attempt_at, created_at LIMIT 100 FOR UPDATE SKIP LOCKED)")).returning(sea_orm::sea_query::Query::returning().exprs([sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("task_id"))), sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("node_id"))), sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("agent_id"))), sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("agent_version"))), sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("delivered")))])).to_string(sea_orm::sea_query::PostgresQueryBuilder)).fetch_all(&self.store.pool).await?;
		let mut deliveries = stream::iter(pending.into_iter().map(|d| async move {
			let result = self.deliver(&d).await;
			(d, result)
		}))
		.buffer_unordered(8);
		while let Some((d, result)) = deliveries.next().await {
			if let Err(e) = result {
				tracing::warn!(task_id=%d.task_id,error=%e,"peer delivery pending");
			}
		}
		Ok(())
	}
	pub async fn authorize_task(&self, node: &str, task_id: Uuid, agent: &EntityRef) -> Result<()> {
		let allowed:bool=sqlx::query_scalar(&sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::Expr::cust("EXISTS(SELECT 1 FROM delegations WHERE task_id = $1 AND node_id = $2 AND agent_id = $3 AND agent_version = $4)")).to_string(sea_orm::sea_query::PostgresQueryBuilder))
            .bind(task_id).bind(node).bind(&agent.id).bind(&agent.version).fetch_one(&self.store.pool).await?;
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
	pub run: Run,
	pub(crate) authority: Option<crate::authorization::execution::WorkerAuthority>,
}

impl Home {
	pub fn new(federation: Federation, run: Run) -> Self {
		Self {
			federation,
			run,
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
	async fn command<T: DeserializeOwned>(&self, op: &str, data: Value) -> Result<T> {
		self.federation.request(&self.run.home_node,Method::POST,"/workspace",Some(&json!({"task_id":self.run.task_id,"agent":{"id":self.run.agent_id,"version":self.run.agent_version},"operation":op,"data":data}))).await
	}
	pub async fn snapshot(&self) -> Result<WorkspaceSnapshot> {
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
		if task.owner.as_deref() == Some(&self.owner()) && task.status != "OPEN" {
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
	pub async fn transition(&self, next: &str) -> Result<Task> {
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
				)
				.await
		} else {
			self.command("complete", json!({"key":key,"artifact":artifact}))
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
			.assign(&self.federation, &self.run, task, policy, reason)
			.await
	}
	pub async fn create_task(&self, key: &str, input: &NewTask) -> Result<Task> {
		if let Some(authority) = &self.authority {
			return authority
				.create_task(&self.federation, &self.run, key, input)
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
		if let Some(authority) = &self.authority {
			return authority
				.delegate(&self.federation, &self.run, task_id, node, agent)
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
				json!({"task_id":task_id,"node_id":node,"agent":agent}),
			)
			.await
		}
	}
	pub async fn message(&self, key: &str, content: &str) -> Result<()> {
		if self.authority.is_some() {
			self.federation
				.store
				.message_from_run(&self.run, &self.owner(), content, key)
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
	pub async fn report(&self, key: &str, kind: &str, data: Value) -> Result<()> {
		if self.local() {
			return Ok(());
		}
		self.command::<Value>("event", json!({"key":key,"kind":kind,"data":data}))
			.await?;
		Ok(())
	}
}

fn map_workspace_chunk_bad_request(
	path: &str,
	request: Option<&Value>,
	error: &Value,
) -> Option<Error> {
	if path != "/workspace"
		|| request.is_none_or(|body| body["operation"] != "workspace_record_chunk")
	{
		return None;
	}
	Some(Error::Invalid(
		error["error"]
			.as_str()
			.unwrap_or("invalid remote workspace record chunk")
			.to_owned(),
	))
}

#[cfg(test)]
mod review_tests {
	use super::*;

	#[test]
	fn remote_workspace_chunk_bad_requests_keep_the_tool_error_type() {
		let request = json!({"operation":"workspace_record_chunk"});
		let error = json!({"error":"workspace record offset out of range"});
		assert!(matches!(
			map_workspace_chunk_bad_request("/workspace", Some(&request), &error),
			Some(Error::Invalid(message)) if message == "workspace record offset out of range"
		));
		assert!(map_workspace_chunk_bad_request("/discover", Some(&request), &error).is_none());
		assert!(map_workspace_chunk_bad_request("/workspace", None, &error).is_none());
	}

	#[test]
	fn child_task_summary_is_independent_of_child_payload_sizes() {
		let mut summary = ChildTaskSummary {
			has_pending: false,
			has_failed: false,
		};
		for status in ["RUNNING", "FAILED"] {
			summary.include_status(status);
		}
		assert!(summary.has_pending && summary.has_failed);
		assert!(serde_json::to_vec(&summary).unwrap().len() < 64);
	}
}
