pub mod api;
pub(crate) mod budget;
pub mod lifecycle;
pub mod policy;
pub mod provision;

use crate::{
    Error, Result,
    authorization::{
        access::Access,
        catalog, execution,
        identity::SubjectIdentity,
        policy::{Resource, SubjectKind},
    },
    domain::{Task, qualified_agent},
    federation::{Delegation, Federation},
    registry::{AgentConfig, EntityRef, Entry, Search},
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
#[schema(as = GenerationRequest)]
pub struct Request {
    pub id: Uuid,
    pub tenant: String,
    pub policy_id: String,
    pub policy_revision: i64,
    pub task_id: Uuid,
    pub workspace_id: Uuid,
    #[serde(skip_serializing)]
    #[schema(ignore)]
    pub(crate) credential_id: Uuid,
    pub root_subject: String,
    pub subject_chain: Vec<String>,
    pub agent_id: String,
    pub agent_version: String,
    #[schema(value_type=Entry)]
    pub definition: Value,
    pub status: String,
    pub reason: String,
    pub depth: i32,
    pub token_limit: i64,
    pub quota_released: bool,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}
#[derive(Serialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[schema(as = GenerationAssignment)]
pub enum Assignment {
    Existing { delegation: Delegation },
    Generated { generation: Box<Request> },
}

pub(crate) fn resource(access: &Access, id: &str) -> Resource {
    access.resource("generation_policy", id, json!({}))
}

pub async fn assign(
    f: &Federation,
    identity: &SubjectIdentity,
    task_id: Uuid,
    policy_id: &str,
    reason: &str,
) -> Result<Assignment> {
    let mut access = Access::begin(&f.store, identity).await?;
    let result = assign_in(f, &mut access, task_id, policy_id, reason).await;
    let assignment = access.finish(result).await?;
    f.notify.notify_waiters();
    Ok(assignment)
}

pub(crate) async fn assign_in(
    f: &Federation,
    access: &mut Access,
    task_id: Uuid,
    policy_id: &str,
    reason: &str,
) -> Result<Assignment> {
    crate::domain::nonempty(reason, "generation reason")?;
    if reason.len() > 4096 {
        return Err(Error::Invalid(
            "generation reason exceeds 4096 bytes".into(),
        ));
    }
    let task: Task = sqlx::query_as("SELECT * FROM tasks WHERE id=$1 FOR UPDATE")
        .bind(task_id)
        .fetch_optional(&mut *access.tx)
        .await?
        .ok_or(Error::Forbidden)?;
    let workspace = access.workspace(task.workspace_id).await?;
    access.context = workspace.attributes.clone();
    if !execution::inherit_task_origin(access, task_id).await?
        && (task.created_by != access.identity.subject || access.subjects.len() > 1)
    {
        return Err(Error::Forbidden);
    }
    access.require(&workspace, "workspace.read").await?;
    access
        .require(
            &access.resource("task", task_id, json!({"created_by":task.created_by})),
            "task.read",
        )
        .await?;
    access
        .require(
            &access.resource("task", task_id, json!({"created_by":task.created_by})),
            "task.delegate",
        )
        .await?;
    access
        .require(&resource(access, policy_id), "generation.request")
        .await?;
    access
        .require(&resource(access, policy_id), "generation.read")
        .await?;
    // The policy lock serializes quota reservations and identical task retries.
    let policy = policy::load(&mut access.tx, &access.identity.tenant, policy_id, true).await?;
    let existing: Option<Request> =
        sqlx::query_as("SELECT * FROM generation_requests WHERE task_id=$1")
            .bind(task_id)
            .fetch_optional(&mut *access.tx)
            .await?;
    if let Some(existing) = existing {
        if existing.tenant != access.identity.tenant
            || existing.root_subject != access.identity.subject
            || existing.policy_id != policy_id
            || existing.reason != reason
            || existing.subject_chain != access.subjects
        {
            return Err(Error::Conflict(
                "task already has a different generation request".into(),
            ));
        }
        if !existing.visible(access).await? {
            return Err(Error::Forbidden);
        }
        return Ok(Assignment::Generated {
            generation: Box::new(existing),
        });
    }
    if task.status != "OPEN" {
        let existing:Option<(String,String,Vec<String>)>=sqlx::query_as("SELECT r.agent_id,r.agent_version,e.subject_chain FROM authorization_execution e JOIN runs r ON r.id=e.run_id WHERE e.task_id=$1 AND e.tenant=$2 AND e.root_subject=$3")
            .bind(task_id).bind(&access.identity.tenant).bind(&access.identity.subject).fetch_optional(&mut *access.tx).await?;
        if let Some((id, version, chain)) = existing {
            let mut expected = access.subjects.clone();
            expected.push(qualified_agent(&f.config.node_id, &id, &version));
            if chain == expected {
                access.subjects = chain;
                catalog::entry(
                    access,
                    &EntityRef {
                        id: id.clone(),
                        version: version.clone(),
                    },
                    "agent.execute",
                )
                .await?;
                return Ok(Assignment::Existing {
                    delegation: Delegation {
                        task_id,
                        node_id: f.config.node_id.clone(),
                        agent_id: id,
                        agent_version: version,
                        delivered: true,
                    },
                });
            }
        }
        return Err(Error::Conflict("task is already assigned".into()));
    }
    let mut search: Search = serde_json::from_value(task.requirements.clone())?;
    search.kind = Some("agent".into());
    for entry in catalog::list_in(access, &search).await? {
        let generated:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM generation_requests WHERE agent_id=$1 AND agent_version=$2)")
            .bind(&entry.id).bind(&entry.version).fetch_one(&mut *access.tx).await?;
        if generated {
            continue;
        }
        let subject = qualified_agent(&f.config.node_id, &entry.id, &entry.version);
        if access
            .snapshot
            .bundle
            .subjects
            .get(&subject)
            .is_none_or(|s| !s.enabled || s.kind != SubjectKind::Agent)
        {
            continue;
        }
        access.subjects.push(subject);
        let allowed = async {
            Ok::<bool, Error>(
                access.decide(&workspace, "workspace.read").await?
                    && access
                        .decide(&catalog::resource(access, &entry), "agent.execute")
                        .await?
                    && access
                        .decide(&access.resource("task", task_id, json!({})), "task.execute")
                        .await?,
            )
        }
        .await;
        access.subjects.pop();
        if allowed? {
            let delegation = execution::delegate_in(
                f,
                access,
                task_id,
                &EntityRef {
                    id: entry.id,
                    version: entry.version,
                },
            )
            .await?;
            return Ok(Assignment::Existing { delegation });
        }
    }
    if !policy.spec.enabled {
        return Err(Error::Forbidden);
    }
    let config: AgentConfig = policy.spec.validate(&access.snapshot.bundle)?;
    for (reference, action) in std::iter::once((&config.model, "model.infer"))
        .chain(config.tools.iter().map(|r| (r, "tool.invoke")))
        .chain(config.skills.iter().map(|r| (r, "skill.use")))
        .chain(config.cluster.iter().map(|r| (r, "cluster.execute")))
    {
        catalog::entry(access, reference, "registry.read").await?;
        catalog::entry(access, reference, action).await?;
    }
    let previous_depth:Option<i32>=sqlx::query_scalar("SELECT max(depth) FROM generation_requests WHERE tenant=$1 AND ($2 || '/agents/' || agent_id || '@' || agent_version)=ANY($3)")
        .bind(&access.identity.tenant).bind(&f.config.node_id).bind(&access.subjects).fetch_one(&mut *access.tx).await?;
    let depth = previous_depth.unwrap_or(0) + 1;
    let limits = &policy.spec.limits;
    let active:i64=sqlx::query_scalar("SELECT count(*) FROM generation_requests WHERE tenant=$1 AND policy_id=$2 AND status IN ('PENDING_APPROVAL','QUEUED','ACTIVE')")
        .bind(&access.identity.tenant).bind(policy_id).fetch_one(&mut *access.tx).await?;
    if policy.generated_count >= limits.max_agents
        || active >= limits.max_concurrent
        || depth > limits.max_depth
        || access.subjects.len() >= 32
        || policy
            .allocated_tokens
            .checked_add(limits.tokens_per_agent)
            .is_none_or(|n| n > limits.token_budget)
    {
        return Err(Error::Conflict(
            "generation count, concurrency, depth or token budget exceeded".into(),
        ));
    }
    let id = Uuid::new_v4();
    let mut definition = policy.spec.template.clone();
    definition.id = format!("generated-{}", id.simple());
    if !definition.tags.iter().any(|t| t == "generated") {
        definition.tags.push("generated".into());
    }
    crate::registry::validate(&definition)?;
    if !search.matches(&definition) {
        return Err(Error::Invalid(
            "generation template does not satisfy task requirements".into(),
        ));
    }
    let status = if policy.spec.approval_required {
        "PENDING_APPROVAL"
    } else {
        "QUEUED"
    };
    let generated:Request=sqlx::query_as("INSERT INTO generation_requests(id,tenant,policy_id,policy_revision,task_id,workspace_id,credential_id,root_subject,subject_chain,agent_id,agent_version,definition,status,reason,depth,token_limit,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,clock_timestamp()+make_interval(secs=>$17)) RETURNING *")
        .bind(id).bind(&access.identity.tenant).bind(policy_id).bind(policy.revision).bind(task_id).bind(task.workspace_id).bind(access.identity.credential_id).bind(&access.identity.subject).bind(&access.subjects)
        .bind(&definition.id).bind(&definition.version).bind(json!(definition)).bind(status).bind(reason).bind(depth).bind(limits.tokens_per_agent).bind(limits.lifetime_seconds as f64).fetch_one(&mut *access.tx).await?;
    sqlx::query("UPDATE generation_policies SET generated_count=generated_count+1,allocated_tokens=allocated_tokens+$3 WHERE tenant=$1 AND id=$2")
        .bind(&access.identity.tenant).bind(policy_id).bind(limits.tokens_per_agent).execute(&mut *access.tx).await?;
    sqlx::query("INSERT INTO generation_budgets(request_id,token_limit) VALUES($1,$2)")
        .bind(id)
        .bind(limits.tokens_per_agent)
        .execute(&mut *access.tx)
        .await?;
    sqlx::query(
        "INSERT INTO generation_history(request_id,status,actor,reason) VALUES($1,$2,$3,$4)",
    )
    .bind(id)
    .bind(status)
    .bind(&access.identity.subject)
    .bind(reason)
    .execute(&mut *access.tx)
    .await?;
    f.store
        .event(
            &mut access.tx,
            Some(task.workspace_id),
            "generation.requested",
            json!({"id":id,"task_id":task_id,"policy_id":policy_id,"status":status}),
        )
        .await?;
    if !generated.visible(access).await? {
        return Err(Error::Forbidden);
    }
    Ok(Assignment::Generated {
        generation: Box::new(generated),
    })
}
