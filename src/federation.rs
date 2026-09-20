use crate::{
    Error, Result,
    config::{Config, PROTOCOL_VERSION, peer_secret, validate_endpoint, validate_node_id},
    domain::*,
    registry::{AgentConfig, EntityRef, Entry, Registry, Search},
    store::Store,
};
use reqwest::Method;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
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
        peer_secret(&peer.credential_env)?;
        let identity: Value = self
            .client
            .get(format!(
                "{}/.well-known/aidash",
                peer.endpoint.trim_end_matches('/')
            ))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if identity["id"] != peer.node_id || identity["protocol_version"] != PROTOCOL_VERSION {
            return Err(Error::Invalid(
                "peer identity or protocol does not match".into(),
            ));
        }
        sqlx::query("INSERT INTO peers(node_id,endpoint,credential_env,protocol_version,enabled) VALUES($1,$2,$3,$4,$5) ON CONFLICT(node_id) DO UPDATE SET endpoint=EXCLUDED.endpoint,credential_env=EXCLUDED.credential_env,protocol_version=EXCLUDED.protocol_version,enabled=EXCLUDED.enabled")
            .bind(&peer.node_id).bind(&peer.endpoint).bind(&peer.credential_env).bind(&peer.protocol_version).bind(peer.enabled).execute(&self.store.pool).await?;
        self.store
            .emit(
                None,
                "peer.registered",
                json!({"node_id":peer.node_id,"endpoint":peer.endpoint,"enabled":peer.enabled}),
            )
            .await?;
        Ok(peer)
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
        Ok(response.json().await?)
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
        for peer in self.peers().await?.into_iter().filter(|p| p.enabled) {
            match self
                .request::<Vec<Entry>>(
                    &peer.node_id,
                    Method::POST,
                    "/discover",
                    Some(&json!(query)),
                )
                .await
            {
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
            self.peer(node).await?;
        }
        let mut tx = self.store.pool.begin().await?;
        sqlx::query("INSERT INTO delegations(task_id,node_id,agent_id,agent_version) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING")
            .bind(task_id).bind(node).bind(&agent.id).bind(&agent.version).execute(&mut *tx).await?;
        let d:Delegation=sqlx::query_as("SELECT task_id,node_id,agent_id,agent_version,delivered FROM delegations WHERE task_id=$1").bind(task_id).fetch_one(&mut *tx).await?;
        if d.node_id != node || d.agent_id != agent.id || d.agent_version != agent.version {
            return Err(Error::Conflict(
                "task already delegated to a different agent".into(),
            ));
        }
        self.store
            .event(&mut tx, Some(task.workspace_id), "task.delegated", json!(d))
            .await?;
        tx.commit().await?;
        // Durable delivery is retried by the server if this immediate attempt fails.
        if let Err(e) = self.deliver(&d).await {
            tracing::warn!(error=%e,task_id=%task_id,"delegation queued for retry");
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
        sqlx::query("UPDATE delegations SET delivered=true WHERE task_id=$1")
            .bind(d.task_id)
            .execute(&self.store.pool)
            .await?;
        Ok(())
    }
    pub async fn retry_deliveries(&self) -> Result<()> {
        let pending:Vec<Delegation>=sqlx::query_as("SELECT task_id,node_id,agent_id,agent_version,delivered FROM delegations WHERE NOT delivered ORDER BY created_at LIMIT 100").fetch_all(&self.store.pool).await?;
        for d in pending {
            if let Err(e) = self.deliver(&d).await {
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
                .complete(self.run.task_id, &self.owner(), key, artifact)
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
                .publish_artifact(self.run.task_id, &self.owner(), key, artifact)
                .await
        } else {
            self.command("artifact", json!({"key":key,"artifact":artifact}))
                .await
        }
    }
    pub async fn create_task(&self, key: &str, input: &NewTask) -> Result<Task> {
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
        if self.local() {
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
