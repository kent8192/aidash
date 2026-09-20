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
            registry: Registry::new(store.pool.clone()),
            store,
            ..self.clone()
        })
    }

    pub async fn peers(&self) -> Result<Vec<Peer>> {
        Ok(sqlx::query_as("SELECT * FROM peers ORDER BY node_id")
            .fetch_all(&self.store.pool)
            .await?)
    }
    pub async fn peer(&self, node: &str) -> Result<Peer> {
        sqlx::query_as("SELECT * FROM peers WHERE node_id=$1 AND enabled")
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
            let existing: Peer =
                sqlx::query_as("UPDATE peers SET enabled=false WHERE node_id=$1 RETURNING *")
                    .bind(&peer.node_id)
                    .fetch_optional(&mut *tx)
                    .await?
                    .ok_or_else(|| Error::NotFound("peer".into()))?;
            self.store.event(&mut tx, None, "peer.registered", json!({"node_id":existing.node_id,"endpoint":existing.endpoint,"enabled":false})).await?;
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
        sqlx::query("SELECT pg_advisory_xact_lock(71003203)")
            .execute(&mut *tx)
            .await?;
        let peers: Vec<Peer> = sqlx::query_as("SELECT * FROM peers WHERE enabled AND node_id<>$1")
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
        sqlx::query("INSERT INTO peers(node_id,endpoint,credential_env,protocol_version,enabled) VALUES($1,$2,$3,$4,$5) ON CONFLICT(node_id) DO UPDATE SET endpoint=EXCLUDED.endpoint,credential_env=EXCLUDED.credential_env,protocol_version=EXCLUDED.protocol_version,enabled=EXCLUDED.enabled")
            .bind(&peer.node_id).bind(&peer.endpoint).bind(&peer.credential_env).bind(&peer.protocol_version).bind(peer.enabled).execute(&mut *tx).await?;
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
        let response = request.send().await?;
        let status = response.status();
        if !status.is_success() {
            return Err(if status == reqwest::StatusCode::CONFLICT {
                Error::Conflict("remote task state changed".into())
            } else {
                Error::External(format!("peer {node} returned {status}"))
            });
        }
        crate::response::json(response, 4_194_304).await
    }
    pub async fn discover(&self, search: &Search) -> Result<Discovery> {
        let mut query = search.clone();
        query.kind = Some("agent".into());
        let mut result = Discovery {
            agents: self
                .registry
                .list(&query)
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
                let response = self
                    .request::<Vec<Entry>>(
                        &peer.node_id,
                        Method::POST,
                        "/discover",
                        Some(&json!(query)),
                    )
                    .await;
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
            let entries: Vec<Entry> = self
                .request(node, Method::POST, "/discover", Some(&json!(search)))
                .await?;
            if !entries.iter().any(|entry| {
                entry.id == agent.id && entry.version == agent.version && search.matches(entry)
            }) {
                return Err(Error::Invalid(
                    "remote agent is missing or does not satisfy task requirements".into(),
                ));
            }
        }
        let mut tx = self.store.pool.begin().await?;
        let d = self.delegate_in(&mut tx, &task, node, agent).await?;
        tx.commit().await?;
        if let Err(e) = self.deliver(&d).await {
            tracing::warn!(error=%e,task_id=%task_id,"delegation queued for retry");
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
        let task_id = task.id;
        sqlx::query("INSERT INTO delegations(task_id,node_id,agent_id,agent_version) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING")
            .bind(task_id).bind(node).bind(&agent.id).bind(&agent.version).execute(&mut **tx).await?;
        let d:Delegation=sqlx::query_as("SELECT task_id,node_id,agent_id,agent_version,delivered FROM delegations WHERE task_id=$1").bind(task_id).fetch_one(&mut **tx).await?;
        if d.node_id != node || d.agent_id != agent.id || d.agent_version != agent.version {
            return Err(Error::Conflict(
                "task already delegated to a different agent".into(),
            ));
        }
        self.store
            .event(tx, Some(task.workspace_id), "task.delegated", json!(d))
            .await?;
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
        sqlx::query("UPDATE delegations SET delivered=true WHERE task_id=$1")
            .bind(d.task_id)
            .execute(&self.store.pool)
            .await?;
        Ok(())
    }
    pub async fn retry_deliveries(&self) -> Result<()> {
        let pending:Vec<Delegation>=sqlx::query_as("UPDATE delegations SET next_attempt_at=now()+interval '5 seconds' WHERE task_id IN (SELECT task_id FROM delegations WHERE NOT delivered AND next_attempt_at<=now() ORDER BY next_attempt_at,created_at LIMIT 100 FOR UPDATE SKIP LOCKED) RETURNING task_id,node_id,agent_id,agent_version,delivered").fetch_all(&self.store.pool).await?;
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
        let allowed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM delegations WHERE task_id=$1 AND node_id=$2 AND agent_id=$3 AND agent_version=$4)")
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
            self.command("snapshot", json!({})).await
        }
    }
    pub async fn task(&self) -> Result<Task> {
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
