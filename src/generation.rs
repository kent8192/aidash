pub mod api;
pub(crate) mod budget;
pub(crate) mod compaction;
pub(crate) mod embedding;
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
    let task: Task = sqlx::query_as(
        &sea_orm::sea_query::Query::select()
            .expr(sea_orm::sea_query::SimpleExpr::from(
                sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
            ))
            .from(sea_orm::sea_query::Alias::new("tasks"))
            .and_where(sea_orm::sea_query::Expr::cust("id = $1"))
            .lock(sea_orm::sea_query::LockType::Update)
            .to_string(sea_orm::sea_query::PostgresQueryBuilder),
    )
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
    let task_resource = access.task_resource(&task).await?;
    access.require(&task_resource, "task.read").await?;
    access.require(&task_resource, "task.delegate").await?;
    access
        .require(&resource(access, policy_id), "generation.request")
        .await?;
    access
        .require(&resource(access, policy_id), "generation.read")
        .await?;
    // The policy lock serializes quota reservations and identical task retries.
    let policy = policy::load(&mut access.tx, &access.identity.tenant, policy_id, true).await?;
    let existing: Option<Request> = sqlx::query_as(
        &sea_orm::sea_query::Query::select()
            .expr(sea_orm::sea_query::SimpleExpr::from(
                sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
            ))
            .from(sea_orm::sea_query::Alias::new("generation_requests"))
            .and_where(sea_orm::sea_query::Expr::cust("task_id = $1"))
            .to_string(sea_orm::sea_query::PostgresQueryBuilder),
    )
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
        let existing: Option<(String, String, Vec<String>)> = sqlx::query_as(
            &sea_orm::sea_query::Query::select()
                .expr(sea_orm::sea_query::SimpleExpr::from(
                    sea_orm::sea_query::Expr::col((
                        sea_orm::sea_query::Alias::new("r"),
                        sea_orm::sea_query::Alias::new("agent_id"),
                    )),
                ))
                .expr(sea_orm::sea_query::SimpleExpr::from(
                    sea_orm::sea_query::Expr::col((
                        sea_orm::sea_query::Alias::new("r"),
                        sea_orm::sea_query::Alias::new("agent_version"),
                    )),
                ))
                .expr(sea_orm::sea_query::SimpleExpr::from(
                    sea_orm::sea_query::Expr::col((
                        sea_orm::sea_query::Alias::new("e"),
                        sea_orm::sea_query::Alias::new("subject_chain"),
                    )),
                ))
                .from_as(
                    sea_orm::sea_query::Alias::new("authorization_execution"),
                    sea_orm::sea_query::Alias::new("e"),
                )
                .join_as(
                    sea_orm::sea_query::JoinType::InnerJoin,
                    sea_orm::sea_query::Alias::new("runs"),
                    sea_orm::sea_query::Alias::new("r"),
                    sea_orm::sea_query::Expr::cust("r.id = e.run_id"),
                )
                .and_where(sea_orm::sea_query::Expr::cust(
                    "e.task_id = $1 AND e.tenant = $2 AND e.root_subject = $3",
                ))
                .to_string(sea_orm::sea_query::PostgresQueryBuilder),
        )
        .bind(task_id)
        .bind(&access.identity.tenant)
        .bind(&access.identity.subject)
        .fetch_optional(&mut *access.tx)
        .await?;
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
        let generated:bool=sqlx::query_scalar(&sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::Expr::cust("EXISTS(SELECT 1 FROM generation_requests WHERE agent_id = $1 AND agent_version = $2)")).to_string(sea_orm::sea_query::PostgresQueryBuilder))
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
                    && access.decide(&task_resource, "task.read").await?
                    && access.decide(&task_resource, "task.execute").await?,
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
        .chain(
            policy
                .spec
                .compaction
                .iter()
                .map(|c| (&c.provider, "compaction.invoke")),
        )
        .chain(
            policy
                .spec
                .embedding
                .iter()
                .map(|c| (&c.provider, "embedding.invoke")),
        )
    {
        catalog::entry(access, reference, "registry.read").await?;
        catalog::entry(access, reference, action).await?;
    }
    let previous_depth: Option<i32> = sqlx::query_scalar(
        &sea_orm::sea_query::Query::select()
            .expr(sea_orm::sea_query::Expr::cust("MAX(depth)"))
            .from(sea_orm::sea_query::Alias::new("generation_requests"))
            .and_where(sea_orm::sea_query::Expr::cust(
                "tenant = $1 AND ($2 || '/agents/' || agent_id || '@' || agent_version) = ANY($3)",
            ))
            .to_string(sea_orm::sea_query::PostgresQueryBuilder),
    )
    .bind(&access.identity.tenant)
    .bind(&f.config.node_id)
    .bind(&access.subjects)
    .fetch_one(&mut *access.tx)
    .await?;
    let depth = previous_depth.unwrap_or(0) + 1;
    let limits = &policy.spec.limits;
    let active:i64=sqlx::query_scalar(&sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::Expr::cust("COUNT(*)")).from(sea_orm::sea_query::Alias::new("generation_requests")).and_where(sea_orm::sea_query::Expr::cust("tenant = $1 AND policy_id = $2 AND status IN ('PENDING_APPROVAL', 'QUEUED', 'ACTIVE')")).to_string(sea_orm::sea_query::PostgresQueryBuilder))
        .bind(&access.identity.tenant).bind(policy_id).fetch_one(&mut *access.tx).await?;
    let compaction_calls = policy
        .spec
        .compaction
        .as_ref()
        .map_or(0, |c| c.calls_per_agent);
    let embedding_calls = policy
        .spec
        .embedding
        .as_ref()
        .map_or(0, |c| c.calls_per_agent);
    if policy.spec.compaction.as_ref().is_some_and(|c| {
        policy
            .allocated_compaction_calls
            .checked_add(c.calls_per_agent)
            .is_none_or(|n| n > c.call_budget)
    }) || policy.spec.embedding.as_ref().is_some_and(|c| {
        policy
            .allocated_embedding_calls
            .checked_add(c.calls_per_agent)
            .is_none_or(|n| n > c.call_budget)
    }) || policy.generated_count >= limits.max_agents
        || active >= limits.max_concurrent
        || depth > limits.max_depth
        || access.subjects.len() >= 32
        || policy
            .allocated_tokens
            .checked_add(limits.tokens_per_agent)
            .is_none_or(|n| n > limits.token_budget)
    {
        return Err(Error::Conflict(
            "generation count, concurrency, depth, token, compaction or embedding budget exceeded"
                .into(),
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
    let generated: Request = sqlx::query_as(
        &sea_orm::sea_query::Query::insert()
            .into_table(sea_orm::sea_query::Alias::new("generation_requests"))
            .columns([
                sea_orm::sea_query::Alias::new("id"),
                sea_orm::sea_query::Alias::new("tenant"),
                sea_orm::sea_query::Alias::new("policy_id"),
                sea_orm::sea_query::Alias::new("policy_revision"),
                sea_orm::sea_query::Alias::new("task_id"),
                sea_orm::sea_query::Alias::new("workspace_id"),
                sea_orm::sea_query::Alias::new("credential_id"),
                sea_orm::sea_query::Alias::new("root_subject"),
                sea_orm::sea_query::Alias::new("subject_chain"),
                sea_orm::sea_query::Alias::new("agent_id"),
                sea_orm::sea_query::Alias::new("agent_version"),
                sea_orm::sea_query::Alias::new("definition"),
                sea_orm::sea_query::Alias::new("status"),
                sea_orm::sea_query::Alias::new("reason"),
                sea_orm::sea_query::Alias::new("depth"),
                sea_orm::sea_query::Alias::new("token_limit"),
                sea_orm::sea_query::Alias::new("expires_at"),
            ])
            .values_panic([
                sea_orm::sea_query::Expr::cust("$1"),
                sea_orm::sea_query::Expr::cust("$2"),
                sea_orm::sea_query::Expr::cust("$3"),
                sea_orm::sea_query::Expr::cust("$4"),
                sea_orm::sea_query::Expr::cust("$5"),
                sea_orm::sea_query::Expr::cust("$6"),
                sea_orm::sea_query::Expr::cust("$7"),
                sea_orm::sea_query::Expr::cust("$8"),
                sea_orm::sea_query::Expr::cust("$9"),
                sea_orm::sea_query::Expr::cust("$10"),
                sea_orm::sea_query::Expr::cust("$11"),
                sea_orm::sea_query::Expr::cust("$12"),
                sea_orm::sea_query::Expr::cust("$13"),
                sea_orm::sea_query::Expr::cust("$14"),
                sea_orm::sea_query::Expr::cust("$15"),
                sea_orm::sea_query::Expr::cust("$16"),
                sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP() + MAKE_INTERVAL(secs => $17)"),
            ])
            .returning_all()
            .to_string(sea_orm::sea_query::PostgresQueryBuilder),
    )
    .bind(id)
    .bind(&access.identity.tenant)
    .bind(policy_id)
    .bind(policy.revision)
    .bind(task_id)
    .bind(task.workspace_id)
    .bind(access.identity.credential_id)
    .bind(&access.identity.subject)
    .bind(&access.subjects)
    .bind(&definition.id)
    .bind(&definition.version)
    .bind(json!(definition))
    .bind(status)
    .bind(reason)
    .bind(depth)
    .bind(limits.tokens_per_agent)
    .bind(limits.lifetime_seconds as f64)
    .fetch_one(&mut *access.tx)
    .await?;
    sqlx::query(
        &sea_orm::sea_query::Query::update()
            .table(sea_orm::sea_query::Alias::new("generation_policies"))
            .value(
                sea_orm::sea_query::Alias::new("generated_count"),
                sea_orm::sea_query::Expr::cust("generated_count + 1"),
            )
            .value(
                sea_orm::sea_query::Alias::new("allocated_tokens"),
                sea_orm::sea_query::Expr::cust("allocated_tokens + $3"),
            )
            .value(
                sea_orm::sea_query::Alias::new("allocated_compaction_calls"),
                sea_orm::sea_query::Expr::cust("allocated_compaction_calls + $4"),
            )
            .value(
                sea_orm::sea_query::Alias::new("allocated_embedding_calls"),
                sea_orm::sea_query::Expr::cust("allocated_embedding_calls + $5"),
            )
            .and_where(sea_orm::sea_query::Expr::cust("tenant = $1 AND id = $2"))
            .to_string(sea_orm::sea_query::PostgresQueryBuilder),
    )
    .bind(&access.identity.tenant)
    .bind(policy_id)
    .bind(limits.tokens_per_agent)
    .bind(compaction_calls)
    .bind(embedding_calls)
    .execute(&mut *access.tx)
    .await?;
    sqlx::query(
        &sea_orm::sea_query::Query::insert()
            .into_table(sea_orm::sea_query::Alias::new("generation_budgets"))
            .columns([
                sea_orm::sea_query::Alias::new("request_id"),
                sea_orm::sea_query::Alias::new("token_limit"),
                sea_orm::sea_query::Alias::new("compaction_call_limit"),
                sea_orm::sea_query::Alias::new("embedding_call_limit"),
            ])
            .values_panic([
                sea_orm::sea_query::Expr::cust("$1"),
                sea_orm::sea_query::Expr::cust("$2"),
                sea_orm::sea_query::Expr::cust("$3"),
                sea_orm::sea_query::Expr::cust("$4"),
            ])
            .to_string(sea_orm::sea_query::PostgresQueryBuilder),
    )
    .bind(id)
    .bind(limits.tokens_per_agent)
    .bind(compaction_calls)
    .bind(embedding_calls)
    .execute(&mut *access.tx)
    .await?;
    sqlx::query(
        &sea_orm::sea_query::Query::insert()
            .into_table(sea_orm::sea_query::Alias::new("generation_history"))
            .columns([
                sea_orm::sea_query::Alias::new("request_id"),
                sea_orm::sea_query::Alias::new("status"),
                sea_orm::sea_query::Alias::new("actor"),
                sea_orm::sea_query::Alias::new("reason"),
            ])
            .values_panic([
                sea_orm::sea_query::Expr::cust("$1"),
                sea_orm::sea_query::Expr::cust("$2"),
                sea_orm::sea_query::Expr::cust("$3"),
                sea_orm::sea_query::Expr::cust("$4"),
            ])
            .to_string(sea_orm::sea_query::PostgresQueryBuilder),
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
