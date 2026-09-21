//! Lifecycle mutations take the tenant policy lock exclusively before any job
//! lock. Worker boundaries hold its shared lease, so stop/disable linearizes
//! after in-flight effects and before the next model or tool call.
use super::Request;
use crate::{Error, Result, authorization::access::Access, federation::Federation};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = GenerationAction)]
pub enum Action {
    Approve,
    Deny,
    Stop,
    Delete,
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = GenerationControl)]
pub struct Control {
    pub action: Action,
    pub reason: String,
}
#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
#[schema(as = GenerationHistory)]
pub struct History {
    pub sequence: i64,
    pub request_id: Uuid,
    pub status: String,
    pub actor: String,
    pub reason: String,
    pub created_at: DateTime<Utc>,
}

pub(crate) async fn load(
    tx: &mut Transaction<'_, Postgres>,
    tenant: &str,
    id: Uuid,
) -> Result<Request> {
    sqlx::query_as("SELECT * FROM generation_requests WHERE tenant=$1 AND id=$2 FOR UPDATE")
        .bind(tenant)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(Error::Forbidden)
}
impl Request {
    pub(crate) async fn visible(&self, access: &mut Access) -> Result<bool> {
        if !access.inherited_lease {
            access.context = json!({});
        }
        let workspace = access.workspace(self.workspace_id).await?;
        let task: Option<crate::domain::Task> =
            sqlx::query_as("SELECT * FROM tasks WHERE id=$1 AND workspace_id=$2")
                .bind(self.task_id)
                .bind(self.workspace_id)
                .fetch_optional(&mut *access.tx)
                .await?;
        let Some(task) = task else {
            return Ok(false);
        };
        if !access.task_visible(&task).await? {
            return Ok(false);
        }

        access.context = workspace.attributes.clone();
        Ok(access.decide(&workspace, "workspace.read").await?
            && access
                .decide(&self.resource(access), "generation.read")
                .await?)
    }
    pub(crate) fn resource(&self, access: &Access) -> crate::authorization::policy::Resource {
        access.resource("generation",self.id,json!({"policy_id":self.policy_id,"root_subject":self.root_subject,"task_id":self.task_id}))
    }
}
pub(crate) async fn transition(
    f: &Federation,
    tx: &mut Transaction<'_, Postgres>,
    job: &Request,
    status: &str,
    actor: &str,
    reason: &str,
) -> Result<Request> {
    if matches!(
        status,
        "COMPLETED" | "DENIED" | "STOPPED" | "EXPIRED" | "FAILED" | "DELETED"
    ) && !job.quota_released
    {
        let (unused, unused_calls, unused_embeddings): (i64, i64, i64) = sqlx::query_as(
            "SELECT token_limit-used_tokens,compaction_call_limit-compaction_calls,embedding_call_limit-embedding_calls FROM generation_budgets WHERE request_id=$1 FOR UPDATE",
        )
        .bind(job.id)
        .fetch_one(&mut **tx)
        .await?;
        sqlx::query("UPDATE generation_policies SET allocated_tokens=allocated_tokens-$3,allocated_compaction_calls=allocated_compaction_calls-$4,allocated_embedding_calls=allocated_embedding_calls-$5 WHERE tenant=$1 AND id=$2")
            .bind(&job.tenant).bind(&job.policy_id).bind(unused).bind(unused_calls).bind(unused_embeddings).execute(&mut **tx).await?;
        sqlx::query("UPDATE generation_requests SET quota_released=true WHERE id=$1")
            .bind(job.id)
            .execute(&mut **tx)
            .await?;
    }
    if matches!(status, "STOPPED" | "EXPIRED" | "DELETED") {
        sqlx::query("UPDATE runs SET control='CANCELLED' WHERE task_id=$1 AND phase NOT IN ('COMPLETED','FAILED','CANCELLED')")
            .bind(job.task_id).execute(&mut **tx).await?;
    }
    if matches!(
        status,
        "COMPLETED" | "DENIED" | "STOPPED" | "EXPIRED" | "FAILED" | "DELETED"
    ) {
        let mut snapshot =
            crate::authorization::Authorization::load_with_mode(tx, &job.tenant, true).await?;
        let subject =
            crate::domain::qualified_agent(&f.config.node_id, &job.agent_id, &job.agent_version);
        if let Some(subject) = snapshot.bundle.subjects.get_mut(&subject)
            && subject.enabled
        {
            subject.enabled = false;
            snapshot.revision = snapshot
                .revision
                .checked_add(1)
                .ok_or_else(|| Error::Invalid("authorization revision exhausted".into()))?;
            sqlx::query("UPDATE authorization_bundles SET revision=$2,document=$3,updated_at=now() WHERE tenant=$1")
                .bind(&job.tenant).bind(snapshot.revision).bind(json!(snapshot.bundle)).execute(&mut **tx).await?;
            sqlx::query("INSERT INTO authorization_revisions(tenant,revision,document,actor) VALUES($1,$2,$3,$4)")
                .bind(&job.tenant).bind(snapshot.revision).bind(json!(snapshot.bundle)).bind(actor).execute(&mut **tx).await?;
        }
        let revision:Option<i64>=sqlx::query_scalar("UPDATE authorization_catalog SET enabled=false,revision=revision+1 WHERE tenant=$1 AND entry_id=$2 AND entry_version=$3 AND enabled RETURNING revision")
            .bind(&job.tenant).bind(&job.agent_id).bind(&job.agent_version).fetch_optional(&mut **tx).await?;
        if let Some(revision) = revision {
            sqlx::query("INSERT INTO authorization_catalog_history(tenant,entry_id,entry_version,revision,enabled,actor) VALUES($1,$2,$3,$4,false,$5)")
                .bind(&job.tenant).bind(&job.agent_id).bind(&job.agent_version).bind(revision).bind(actor).execute(&mut **tx).await?;
        }
    }
    let updated =
        sqlx::query_as("UPDATE generation_requests SET status=$2 WHERE id=$1 RETURNING *")
            .bind(job.id)
            .bind(status)
            .fetch_one(&mut **tx)
            .await?;
    sqlx::query(
        "INSERT INTO generation_history(request_id,status,actor,reason) VALUES($1,$2,$3,$4)",
    )
    .bind(job.id)
    .bind(status)
    .bind(actor)
    .bind(reason)
    .execute(&mut **tx)
    .await?;
    f.store
        .event(
            tx,
            Some(job.workspace_id),
            "generation.changed",
            json!({"id":job.id,"task_id":job.task_id,"policy_id":job.policy_id,"status":status}),
        )
        .await?;
    Ok(updated)
}
pub(crate) async fn control(
    f: &Federation,
    tx: &mut Transaction<'_, Postgres>,
    job: &Request,
    input: &Control,
    actor: &str,
) -> Result<Request> {
    crate::domain::nonempty(&input.reason, "control reason")?;
    if input.reason.len() > 4096 {
        return Err(Error::Invalid("control reason exceeds 4096 bytes".into()));
    }
    let status = match input.action {
        Action::Approve => "QUEUED",
        Action::Deny => "DENIED",
        Action::Stop => "STOPPED",
        Action::Delete => "DELETED",
    };
    let replay:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM generation_history WHERE request_id=$1 AND status=$2 AND actor=$3 AND reason=$4)")
        .bind(job.id).bind(status).bind(actor).bind(&input.reason).fetch_one(&mut **tx).await?;
    if replay {
        return load(tx, &job.tenant, job.id).await;
    }
    if job.status == status {
        return Err(Error::Conflict(
            "generation decision already recorded".into(),
        ));
    }
    let active = matches!(
        job.status.as_str(),
        "PENDING_APPROVAL" | "QUEUED" | "ACTIVE"
    );
    let valid = match input.action {
        Action::Approve | Action::Deny => job.status == "PENDING_APPROVAL",
        Action::Stop => active,
        Action::Delete => !active,
    };
    if !valid {
        return Err(Error::Conflict(
            "invalid generation state transition".into(),
        ));
    }
    if matches!(input.action, Action::Approve) && job.expires_at <= Utc::now() {
        return Err(Error::Conflict("generation request expired".into()));
    }
    transition(f, tx, job, status, actor, &input.reason).await
}
