use super::{access::Access, catalog, identity::SubjectIdentity, policy::SubjectKind};
use crate::{
    Error, Result,
    api_schema::RunDetails,
    domain::*,
    federation::{Delegation, DiscoveredAgent, Discovery, Federation},
    provider::ToolCall,
    registry::{AgentConfig, EntityRef, Search},
    store::{Invocation, Store},
    tool::ToolConfig,
};
use sea_orm::sea_query::{
    Alias, Asterisk, Condition, Expr, LockType, OnConflict, Order, PostgresQueryBuilder, Query,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Clone, sqlx::FromRow)]
struct Grant {
    run_id: Uuid,
    task_id: Uuid,
    workspace_id: Uuid,
    tenant: String,
    credential_id: Uuid,
    root_subject: String,
    subject_chain: Vec<String>,
}

impl Grant {
    fn identity(&self) -> SubjectIdentity {
        SubjectIdentity {
            credential_id: self.credential_id,
            tenant: self.tenant.clone(),
            subject: self.root_subject.clone(),
        }
    }
}

async fn grant(store: &Store, run: &Run) -> Result<Option<Grant>> {
    let grant: Option<Grant> = sqlx::query_as(
        &Query::select()
            .column(Asterisk)
            .from(Alias::new("authorization_execution"))
            .cond_where(Expr::col(Alias::new("run_id")).eq(Expr::cust("$1")))
            .to_string(PostgresQueryBuilder),
    )
    .bind(run.id)
    .fetch_optional(&store.pool)
    .await?;
    if let Some(grant) = &grant {
        if run.home_node != store.node_id
            || grant.run_id != run.id
            || grant.task_id != run.task_id
            || grant.workspace_id != run.workspace_id
            || grant.subject_chain.first() != Some(&grant.root_subject)
            || grant.subject_chain.last()
                != Some(&qualified_agent(
                    &store.node_id,
                    &run.agent_id,
                    &run.agent_version,
                ))
        {
            return Err(Error::Forbidden);
        }
    } else {
        store.require_legacy_execution(run.workspace_id).await?;
        store
            .require_legacy_agent(&run.agent_id, &run.agent_version)
            .await?;
    }
    Ok(grant)
}

async fn access_for_run(store: &Store, run: &Run, durable_audit: bool) -> Result<Option<Access>> {
    let Some(grant) = grant(store, run).await? else {
        return Ok(None);
    };
    let mut access = Access::begin(store, &grant.identity()).await?;
    // All paths lock policy, credential, then grant in the same order. A resume
    // can rotate the source credential; a step must not use an older binding.
    let current: Grant = sqlx::query_as(
        &Query::select()
            .column(Asterisk)
            .from(Alias::new("authorization_execution"))
            .cond_where(Expr::col(Alias::new("run_id")).eq(Expr::cust("$1")))
            .lock(LockType::Share)
            .to_string(PostgresQueryBuilder),
    )
    .bind(run.id)
    .fetch_one(&mut *access.tx)
    .await?;
    if current.credential_id != grant.credential_id || current.subject_chain != grant.subject_chain
    {
        return Err(Error::External(
            "execution authority changed; retry boundary".into(),
        ));
    }
    access.subjects = grant.subject_chain;
    access.durable_audit = durable_audit;
    access.worker();
    let workspace = access.workspace(run.workspace_id).await?;
    access.context = workspace.attributes.clone();
    access.require(&workspace, "workspace.read").await?;
    Ok(Some(access))
}

fn require_agent(access: &Access, id: &str) -> Result<()> {
    if access
        .snapshot
        .bundle
        .subjects
        .get(id)
        .is_none_or(|s| s.kind != SubjectKind::Agent)
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}

pub(crate) async fn inherit_task_origin(access: &mut Access, task: Uuid) -> Result<bool> {
    let origin: Option<(String, String, Vec<String>)> = sqlx::query_as(
        &Query::select()
            .column(Alias::new("tenant"))
            .column(Alias::new("root_subject"))
            .column(Alias::new("subject_chain"))
            .from(Alias::new("authorization_task_origins"))
            .cond_where(Expr::col(Alias::new("task_id")).eq(Expr::cust("$1")))
            .to_string(PostgresQueryBuilder),
    )
    .bind(task)
    .fetch_optional(&mut *access.tx)
    .await?;
    if let Some((tenant, root, chain)) = origin {
        if tenant != access.identity.tenant
            || root != access.identity.subject
            || (access.subjects.len() > 1 && access.subjects != chain)
        {
            return Err(Error::Forbidden);
        }
        access.subjects = chain;
        Ok(true)
    } else {
        Ok(false)
    }
}

async fn admit(
    f: &Federation,
    access: &mut Access,
    task_id: Uuid,
    revision: Option<i64>,
    agent: &EntityRef,
    delegation: bool,
) -> Result<Task> {
    let task: Task = sqlx::query_as(
        &Query::select()
            .column(Asterisk)
            .from(Alias::new("tasks"))
            .cond_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
            .to_string(PostgresQueryBuilder),
    )
    .bind(task_id)
    .fetch_optional(&mut *access.tx)
    .await?
    .ok_or(Error::Forbidden)?;
    inherit_task_origin(access, task_id).await?;
    let workspace = access.workspace(task.workspace_id).await?;
    access.context = workspace.attributes.clone();
    access.require(&workspace, "workspace.read").await?;
    if task.status != "OPEN" {
        return Err(Error::Conflict("task is already assigned".into()));
    }
    if delegation {
        access
            .require(
                &access.resource("task", task_id, json!({})),
                "task.delegate",
            )
            .await?;
    }
    crate::generation::provision::require_live(access, &f.config.node_id, task_id, agent).await?;
    let subject = qualified_agent(&f.config.node_id, &agent.id, &agent.version);
    require_agent(access, &subject)?;
    if access.subjects.len() >= 32 {
        return Err(Error::Invalid(
            "execution delegation depth exceeds 32".into(),
        ));
    }
    access.subjects.push(subject.clone());
    access.require(&workspace, "workspace.read").await?;
    let entry = catalog::entry(access, agent, "agent.execute").await?;
    if entry.kind != "agent" {
        return Err(Error::Invalid("executor must be an agent".into()));
    }
    access
        .require(&access.resource("task", task_id, json!({})), "task.execute")
        .await?;
    let claimed = f
        .store
        .claim_in(
            &mut access.tx,
            &task,
            revision.unwrap_or(task.revision),
            &subject,
            &entry,
        )
        .await?;
    let run_id: Uuid = sqlx::query_scalar(
        &Query::select()
            .column(Alias::new("id"))
            .from(Alias::new("runs"))
            .cond_where(
                Condition::all()
                    .add(Expr::col(Alias::new("home_node")).eq(Expr::cust("$1")))
                    .add(Expr::col(Alias::new("task_id")).eq(Expr::cust("$2"))),
            )
            .to_string(PostgresQueryBuilder),
    )
    .bind(&f.config.node_id)
    .bind(task.id)
    .fetch_one(&mut *access.tx)
    .await?;
    sqlx::query(
        &Query::insert()
            .into_table(Alias::new("authorization_execution"))
            .columns([
                Alias::new("run_id"),
                Alias::new("task_id"),
                Alias::new("workspace_id"),
                Alias::new("tenant"),
                Alias::new("credential_id"),
                Alias::new("root_subject"),
                Alias::new("subject_chain"),
            ])
            .values_panic([
                Expr::cust("$1").into(),
                Expr::cust("$2").into(),
                Expr::cust("$3").into(),
                Expr::cust("$4").into(),
                Expr::cust("$5").into(),
                Expr::cust("$6").into(),
                Expr::cust("$7").into(),
            ])
            .to_string(PostgresQueryBuilder),
    )
    .bind(run_id)
    .bind(task.id)
    .bind(task.workspace_id)
    .bind(&access.identity.tenant)
    .bind(access.identity.credential_id)
    .bind(&access.identity.subject)
    .bind(&access.subjects)
    .execute(&mut *access.tx)
    .await?;
    Ok(claimed)
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
        return Err(Error::Forbidden);
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
    // Local admission commits task ownership, the run, grant and delegation
    // together. No intermediate unscoped READY run is ever visible to workers.
    let admitted = admit(f, access, task, None, agent, true).await?;
    let delegation: Delegation = sqlx::query_as(
        &Query::insert()
            .into_table(Alias::new("delegations"))
            .columns([
                Alias::new("task_id"),
                Alias::new("node_id"),
                Alias::new("agent_id"),
                Alias::new("agent_version"),
                Alias::new("delivered"),
            ])
            .values_panic([
                Expr::cust("$1").into(),
                Expr::cust("$2").into(),
                Expr::cust("$3").into(),
                Expr::cust("$4").into(),
                Expr::cust("true").into(),
            ])
            .returning(Query::returning().columns([
                Alias::new("task_id"),
                Alias::new("node_id"),
                Alias::new("agent_id"),
                Alias::new("agent_version"),
                Alias::new("delivered"),
            ]))
            .to_string(PostgresQueryBuilder),
    )
    .bind(task)
    .bind(&f.config.node_id)
    .bind(&agent.id)
    .bind(&agent.version)
    .fetch_one(&mut *access.tx)
    .await?;
    f.store
        .event(
            &mut access.tx,
            Some(admitted.workspace_id),
            "task.delegated",
            json!(delegation),
        )
        .await?;
    Ok(delegation)
}

#[derive(Clone)]
pub(crate) struct WorkerAuthority {
    access: Arc<Mutex<Access>>,
}

impl WorkerAuthority {
    pub async fn assign(
        &self,
        f: &Federation,
        run: &Run,
        task: Uuid,
        policy: &str,
        reason: &str,
    ) -> Result<crate::generation::Assignment> {
        let mut lease = self.access.lock().await;
        // Read all potential references under the outer authority lease; the
        // mutation transaction must not wait behind a catalog revoker.
        catalog::list_in(&mut lease, &Search::default()).await?;
        let mut access = Access::under_lease(&lease).await?;
        let result = async {
            let workspace: Option<Uuid> = sqlx::query_scalar(
                &Query::select()
                    .column(Alias::new("workspace_id"))
                    .from(Alias::new("tasks"))
                    .cond_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
                    .to_string(PostgresQueryBuilder),
            )
            .bind(task)
            .fetch_optional(&mut *access.tx)
            .await?;
            if workspace != Some(run.workspace_id) {
                return Err(Error::Forbidden);
            }
            crate::generation::assign_in(f, &mut access, task, policy, reason).await
        }
        .await;
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
        let result = async {
            let workspace = access.workspace(run.workspace_id).await?;
            access.require(&workspace, "task.create").await?;
            let creator = access.subjects.last().ok_or(Error::Forbidden)?.clone();
            let task = f
                .store
                .create_task_in(&mut access.tx, run.workspace_id, input, &creator, Some(key))
                .await?;
            sqlx::query(
                &Query::insert()
                    .into_table(Alias::new("authorization_task_origins"))
                    .columns([
                        Alias::new("task_id"),
                        Alias::new("source_run_id"),
                        Alias::new("tenant"),
                        Alias::new("root_subject"),
                        Alias::new("subject_chain"),
                    ])
                    .values_panic([
                        Expr::cust("$1").into(),
                        Expr::cust("$2").into(),
                        Expr::cust("$3").into(),
                        Expr::cust("$4").into(),
                        Expr::cust("$5").into(),
                    ])
                    .on_conflict(OnConflict::new().do_nothing().to_owned())
                    .to_string(PostgresQueryBuilder),
            )
            .bind(task.id)
            .bind(run.id)
            .bind(&access.identity.tenant)
            .bind(&access.identity.subject)
            .bind(&access.subjects)
            .execute(&mut *access.tx)
            .await?;
            let origin: (Uuid, String, String, Vec<String>) = sqlx::query_as(
                &Query::select()
                    .column(Alias::new("source_run_id"))
                    .column(Alias::new("tenant"))
                    .column(Alias::new("root_subject"))
                    .column(Alias::new("subject_chain"))
                    .from(Alias::new("authorization_task_origins"))
                    .cond_where(Expr::col(Alias::new("task_id")).eq(Expr::cust("$1")))
                    .to_string(PostgresQueryBuilder),
            )
            .bind(task.id)
            .fetch_one(&mut *access.tx)
            .await?;
            if origin
                != (
                    run.id,
                    access.identity.tenant.clone(),
                    access.identity.subject.clone(),
                    access.subjects.clone(),
                )
            {
                return Err(Error::Conflict(
                    "task already has a different origin".into(),
                ));
            }
            Ok(task)
        }
        .await;
        access.finish(result).await
    }

    pub async fn snapshot(&self, workspace: Uuid) -> Result<WorkspaceSnapshot> {
        self.access.lock().await.workspace_snapshot(workspace).await
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
        let result = async {
            if node != f.config.node_id {
                return Err(Error::Forbidden);
            }
            let workspace: Option<Uuid> = sqlx::query_scalar(
                &Query::select()
                    .column(Alias::new("workspace_id"))
                    .from(Alias::new("tasks"))
                    .cond_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
                    .to_string(PostgresQueryBuilder),
            )
            .bind(task)
            .fetch_optional(&mut *access.tx)
            .await?;
            if workspace != Some(run.workspace_id) {
                return Err(Error::Forbidden);
            }
            access
                .require(&access.resource("task", task, json!({})), "task.delegate")
                .await?;
            let existing: Option<Grant> = sqlx::query_as(
                &Query::select()
                    .column(Asterisk)
                    .from(Alias::new("authorization_execution"))
                    .cond_where(Expr::col(Alias::new("task_id")).eq(Expr::cust("$1")))
                    .to_string(PostgresQueryBuilder),
            )
            .bind(task)
            .fetch_optional(&mut *access.tx)
            .await?;
            if let Some(existing) = existing {
                let mut expected = access.subjects.clone();
                expected.push(qualified_agent(node, &agent.id, &agent.version));
                if existing.subject_chain != expected
                    || existing.credential_id != access.identity.credential_id
                {
                    return Err(Error::Conflict(
                        "task already has a different authority".into(),
                    ));
                }
                return Ok(Delegation {
                    task_id: task,
                    node_id: node.into(),
                    agent_id: agent.id.clone(),
                    agent_version: agent.version.clone(),
                    delivered: true,
                });
            }
            delegate_in(f, &mut access, task, agent).await
        }
        .await;
        access.finish(result).await
    }
    pub async fn discover(&self, f: &Federation, search: &Search) -> Result<Discovery> {
        let mut access = self.access.lock().await;
        let mut query = search.clone();
        query.kind = Some("agent".into());
        let entries = catalog::list_in(&mut access, &query).await?;
        Ok(Discovery {
            agents: entries
                .into_iter()
                .map(|entity| DiscoveredAgent {
                    node_id: f.config.node_id.clone(),
                    entity,
                })
                .collect(),
            errors: vec![],
        })
    }
}

pub(crate) struct Guard {
    access: Arc<Mutex<Access>>,
    run: Run,
    agent: AgentConfig,
}

impl Guard {
    pub async fn begin(f: &Federation, run: &Run) -> Result<Option<Self>> {
        let Some(mut access) = access_for_run(&f.store, run, true).await? else {
            return Ok(None);
        };
        if !access.run_visible(run).await? {
            return Err(Error::Forbidden);
        }
        // A cluster conversation remains bound to its approved entry even if
        // the coordinator agent does not repeat that cluster in its own config.
        let clusters: Vec<String> = sqlx::query_scalar(
            &Query::select()
                .column(Alias::new("target"))
                .from(Alias::new("conversations"))
                .cond_where(
                    Condition::all()
                        .add(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$1")))
                        .add(Expr::cust("target_kind='cluster'")),
                )
                .to_string(PostgresQueryBuilder),
        )
        .bind(run.workspace_id)
        .fetch_all(&mut *access.tx)
        .await?;
        for cluster in clusters {
            let (id, version) = cluster.rsplit_once('@').ok_or(Error::Forbidden)?;
            catalog::entry(
                &mut access,
                &EntityRef {
                    id: id.into(),
                    version: version.into(),
                },
                "cluster.execute",
            )
            .await?;
        }
        access
            .require(
                &access.resource("task", run.task_id, json!({})),
                "task.execute",
            )
            .await?;
        let reference = EntityRef {
            id: run.agent_id.clone(),
            version: run.agent_version.clone(),
        };
        crate::generation::provision::require_live(
            &mut access,
            &f.config.node_id,
            run.task_id,
            &reference,
        )
        .await?;
        let entry = catalog::entry(&mut access, &reference, "agent.execute").await?;
        require_agent(
            &access,
            &qualified_agent(&f.config.node_id, &run.agent_id, &run.agent_version),
        )?;
        let agent: AgentConfig = serde_json::from_value(entry.config)?;
        // Registry versions are immutable. Lock each tenant's approval for the
        // step and validate read access before loading prompts/tool definitions.
        for reference in std::iter::once(&agent.model)
            .chain(agent.tools.iter())
            .chain(agent.skills.iter())
            .chain(agent.cluster.iter())
        {
            catalog::entry(&mut access, reference, "registry.read").await?;
        }
        Ok(Some(Self {
            access: Arc::new(Mutex::new(access)),
            run: run.clone(),
            agent,
        }))
    }

    pub fn authority(&self) -> WorkerAuthority {
        WorkerAuthority {
            access: self.access.clone(),
        }
    }
    pub async fn human_read(&self, id: Uuid) -> Result<()> {
        let mut access = self.access.lock().await;
        let request: HumanRequest = sqlx::query_as(
            &Query::select()
                .column(Asterisk)
                .from(Alias::new("human_requests"))
                .cond_where(
                    Condition::all()
                        .add(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
                        .add(Expr::col(Alias::new("run_id")).eq(Expr::cust("$2")))
                        .add(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$3"))),
                )
                .to_string(PostgresQueryBuilder),
        )
        .bind(id)
        .bind(self.run.id)
        .bind(self.run.workspace_id)
        .fetch_optional(&mut *access.tx)
        .await?
        .ok_or(Error::Forbidden)?;
        let resource = access.human_resource(&request).await?;
        access.require(&resource, "human.read").await
    }
    pub async fn action(&self, action: &str, kind: &str, id: impl ToString) -> Result<()> {
        let mut access = self.access.lock().await;
        let attributes = if kind == "memory" {
            json!({"version":self.run.agent_version})
        } else {
            json!({})
        };
        let resource = access.resource(kind, id, attributes);
        access.require(&resource, action).await
    }

    pub fn compactor(&self, f: &Federation) -> crate::generation::compaction::ApprovedCompactor {
        crate::generation::compaction::ApprovedCompactor {
            access: self.access.clone(),
            store: f.store.clone(),
            run: self.run.clone(),
            client: f.client.clone(),
        }
    }

    pub async fn reserve_inference(
        &self,
        store: &Store,
        attempt: Uuid,
        window: usize,
        output: u32,
    ) -> Result<Option<crate::generation::budget::Reservation>> {
        let mut access = self.access.lock().await;
        crate::generation::budget::reserve(&mut access, store, self.run.id, attempt, window, output)
            .await
    }

    pub async fn inference(&self) -> Result<()> {
        let mut access = self.access.lock().await;
        let node: String = access.environment["node_id"]
            .as_str()
            .ok_or(Error::Forbidden)?
            .into();
        crate::generation::provision::require_live(
            &mut access,
            &node,
            self.run.task_id,
            &EntityRef {
                id: self.run.agent_id.clone(),
                version: self.run.agent_version.clone(),
            },
        )
        .await?;
        catalog::entry(&mut access, &self.agent.model, "model.infer").await?;
        for skill in &self.agent.skills {
            catalog::entry(&mut access, skill, "skill.use").await?;
        }
        let resource = access.resource(
            "memory",
            &self.run.agent_id,
            json!({"version":self.run.agent_version}),
        );
        access.require(&resource, "memory.read").await
    }

    pub async fn tool(&self, call: &ToolCall) -> Result<()> {
        let mut access = self.access.lock().await;
        if let Some(index) = call
            .name
            .strip_prefix("plugin_")
            .and_then(|i| i.parse::<usize>().ok())
        {
            let reference = self.agent.tools.get(index).ok_or(Error::Forbidden)?;
            let entry = catalog::entry(&mut access, reference, "tool.invoke").await?;
            if let ToolConfig::Agent { node_id, agent } = serde_json::from_value(entry.config)? {
                if node_id != self.run.home_node {
                    return Err(Error::Forbidden);
                }
                catalog::entry(&mut access, &agent, "agent.execute").await?;
                let resource = access.resource("workspace", self.run.workspace_id, json!({}));
                access.require(&resource, "task.create").await?;
            }
            return Ok(());
        }
        let resource = access.resource("tool", format!("builtin:{}", call.name), json!({}));
        access.require(&resource, "tool.invoke").await?;
        let (action, kind, id) = match call.name.as_str() {
            "task_create" => (
                "task.create",
                "workspace",
                self.run.workspace_id.to_string(),
            ),
            "task_assign" => {
                let id = call.arguments["policy_id"]
                    .as_str()
                    .ok_or_else(|| Error::Invalid("missing generation policy".into()))?;
                ("generation.request", "generation_policy", id.to_owned())
            }
            "task_delegate" => {
                let id = call.arguments["task_id"]
                    .as_str()
                    .and_then(|s| s.parse::<Uuid>().ok())
                    .ok_or_else(|| Error::Invalid("invalid task id".into()))?;
                if call.arguments["node_id"] != self.run.home_node {
                    return Err(Error::Forbidden);
                }
                let reference: EntityRef = serde_json::from_value(call.arguments["agent"].clone())?;
                catalog::entry(&mut access, &reference, "agent.execute").await?;
                ("task.delegate", "task", id.to_string())
            }
            "artifact_publish" => ("artifact.create", "artifact", self.run.task_id.to_string()),
            "workspace_message" => (
                "message.create",
                "workspace",
                self.run.workspace_id.to_string(),
            ),
            "memory_write" => ("memory.write", "memory", self.run.agent_id.clone()),
            "human_request" => ("human.request", "run", self.run.id.to_string()),
            "agent_discover" | "workspace_observe" | "workspace_wait" => return Ok(()),
            _ => return Err(Error::Forbidden),
        };
        let attributes = if kind == "memory" {
            json!({"version":self.run.agent_version})
        } else {
            json!({})
        };
        let resource = access.resource(kind, id, attributes);
        access.require(&resource, action).await
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
    action: &str,
) -> Result<Run> {
    let mut access = Access::begin(&f.store, identity).await?;
    let result = async {
        let run: Run = sqlx::query_as(
            &Query::select()
                .column(Asterisk)
                .from(Alias::new("runs"))
                .cond_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
                .to_string(PostgresQueryBuilder),
        )
        .bind(id)
        .fetch_optional(&mut *access.tx)
        .await?
        .ok_or(Error::Forbidden)?;
        let workspace = access.workspace(run.workspace_id).await?;
        access.context = workspace.attributes.clone();
        access.require(&workspace, "workspace.read").await?;
        // The control response includes the run's persisted context/pending data.
        if !access.run_visible(&run).await? {
            return Err(Error::Forbidden);
        }
        access
            .require(&access.resource("run", id, json!({})), "run.control")
            .await?;
        if action == "resume" {
            let grant: Grant = sqlx::query_as(
                &Query::select()
                    .column(Asterisk)
                    .from(Alias::new("authorization_execution"))
                    .cond_where(Expr::col(Alias::new("run_id")).eq(Expr::cust("$1")))
                    .lock(LockType::Update)
                    .to_string(PostgresQueryBuilder),
            )
            .bind(id)
            .fetch_optional(&mut *access.tx)
            .await?
            .ok_or(Error::Forbidden)?;
            if grant.tenant != identity.tenant || grant.root_subject != identity.subject {
                return Err(Error::Forbidden);
            }
            sqlx::query(
                &Query::update()
                    .table(Alias::new("authorization_execution"))
                    .value(Alias::new("credential_id"), Expr::cust("$2"))
                    .cond_where(Expr::col(Alias::new("run_id")).eq(Expr::cust("$1")))
                    .to_string(PostgresQueryBuilder),
            )
            .bind(id)
            .bind(identity.credential_id)
            .execute(&mut *access.tx)
            .await?;
        }
        f.store.control_in(&mut access.tx, id, action).await
    }
    .await;
    let result = access.finish(result).await?;
    f.notify.notify_waiters();
    Ok(result)
}

pub async fn details(f: &Federation, identity: &SubjectIdentity, id: Uuid) -> Result<RunDetails> {
    let mut access = Access::begin(&f.store, identity).await?;
    let result = async {
        let run: Run = sqlx::query_as(
            &Query::select()
                .column(Asterisk)
                .from(Alias::new("runs"))
                .cond_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
                .to_string(PostgresQueryBuilder),
        )
        .bind(id)
        .fetch_optional(&mut *access.tx)
        .await?
        .ok_or(Error::Forbidden)?;
        let workspace = access.workspace(run.workspace_id).await?;
        access.context = workspace.attributes.clone();
        access.require(&workspace, "workspace.read").await?;
        if !access.run_visible(&run).await? {
            return Err(Error::Forbidden);
        }
        let invocations: Vec<Invocation> = sqlx::query_as(
            &Query::select()
                .column(Asterisk)
                .from(Alias::new("invocations"))
                .cond_where(Expr::col(Alias::new("run_id")).eq(Expr::cust("$1")))
                .order_by(Alias::new("created_at"), Order::Asc)
                .to_string(PostgresQueryBuilder),
        )
        .bind(id)
        .fetch_all(&mut *access.tx)
        .await?;
        let memory: Option<Value> = sqlx::query_scalar(
            &Query::select()
                .column(Alias::new("data"))
                .from(Alias::new("memory"))
                .cond_where(
                    Condition::all()
                        .add(Expr::col(Alias::new("agent_id")).eq(Expr::cust("$1")))
                        .add(Expr::col(Alias::new("agent_version")).eq(Expr::cust("$2")))
                        .add(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$3"))),
                )
                .to_string(PostgresQueryBuilder),
        )
        .bind(&run.agent_id)
        .bind(&run.agent_version)
        .bind(run.workspace_id)
        .fetch_optional(&mut *access.tx)
        .await?;
        Ok(RunDetails {
            run,
            invocations,
            memory: memory.unwrap_or_else(|| json!({})),
        })
    }
    .await;
    access.finish(result).await
}

pub async fn discover(
    f: &Federation,
    identity: &SubjectIdentity,
    search: &Search,
) -> Result<Discovery> {
    let mut query = search.clone();
    query.kind = Some("agent".into());
    let entries = catalog::list(&f.store, identity, &query).await?;
    Ok(Discovery {
        agents: entries
            .into_iter()
            .map(|entity| DiscoveredAgent {
                node_id: f.config.node_id.clone(),
                entity,
            })
            .collect(),
        errors: vec![],
    })
}

pub(crate) async fn cancel_if_scoped(store: &Store, run: &Run, token: Uuid) -> Result<bool> {
    if run.control != "CANCELLED" || grant(store, run).await?.is_none() {
        return Ok(false);
    }
    // Cancellation was already authorized at the control API. It is cleanup,
    // without model/tool calls, and must remain possible after revocation.
    store.cancel_execution(run, token).await?;
    Ok(true)
}
