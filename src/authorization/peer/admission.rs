//! Receiver admission binds a source grant to one local identity and executor.
//! The record is not an executable run: worker activation must consume this
//! authority through scoped home commands, never through legacy offers.
use super::execution::{InspectInput, inspect_in};
use crate::{
    Error, Result,
    authorization::{access::Access, remote::Description},
    federation::Federation,
    registry::EntityRef,
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Input {
    grant_id: Uuid,
}
#[derive(sqlx::FromRow)]
struct Record {
    id: Uuid,
    source_node: String,
    grant_id: Uuid,
    task_id: Uuid,
    tenant: String,
    credential_id: Uuid,
    subject_chain: Vec<String>,
    description: Value,
}
#[derive(Serialize)]
pub(crate) struct Admission {
    id: Uuid,
    source_node: String,
    grant_id: Uuid,
    task_id: Uuid,
    expires_at: DateTime<Utc>,
}
impl Record {
    fn matches(&self, access: &Access, description: &Description) -> Result<bool> {
        Ok(self.source_node == description.source_node
            && self.grant_id == description.grant_id
            && self.task_id == description.task.id
            && self.tenant == access.identity.tenant
            && self.credential_id == access.identity.credential_id
            && self.subject_chain == access.subjects
            && self.description == serde_json::to_value(description)?)
    }
    fn view(&self, description: &Description) -> Admission {
        Admission {
            id: self.id,
            source_node: self.source_node.clone(),
            grant_id: self.grant_id,
            task_id: self.task_id,
            expires_at: description.expires_at,
        }
    }
}

async fn lease(f: &Federation, source: &str, grant: Uuid) -> Result<(Access, Description)> {
    // Do not hold receiver authority while calling home: home's verification
    // inspects this receiver too, and a queued policy writer must not deadlock
    // two nested shared leases. Recheck the receiver under a fresh lease after
    // the source reply and retain it until the local binding is committed.
    let description: Description = super::authority_request(
        f,
        source,
        "/scoped/execution/grants/describe",
        &json!({"grant_id":grant}),
    )
    .await?;
    if description.source_node != source
        || description.target_node != f.config.node_id
        || description.grant_id != grant
        || description.task.status != "OPEN"
    {
        return Err(Error::Forbidden);
    }
    let mut access = super::access(
        f,
        source,
        &description.source_tenant,
        &description.source_subject,
    )
    .await?;
    let result=async {
        access.context["source_workspace_id"]=json!(description.task.workspace_id);
        access.context["source_task_id"]=json!(description.task.id);
        access.context["source_task_revision"]=json!(description.task.revision);
        let fresh=inspect_in(f,&mut access,source,&InspectInput {
            tenant:description.source_tenant.clone(),subject:description.source_subject.clone(),
            agent:EntityRef{id:description.inspection.agent.id.clone(),version:description.inspection.agent.version.clone()},
            requirements:serde_json::from_value(description.task.requirements.clone())?,
        }).await?;
        if fresh!=description.inspection {return Err(Error::Forbidden);}
        let workspace=access.resource("workspace",format!("{source}/workspaces/{}",description.task.workspace_id),json!({}));
        access.require(&workspace,"workspace.read").await?;
        let task=access.resource("task",format!("{source}/tasks/{}",description.task.id),json!({"created_by":description.task.created_by,"requirements":description.task.requirements}));
        access.require(&task,"task.read").await?;
        access.require(&task,"task.execute").await?;
        let live:bool=sqlx::query_scalar("SELECT $1::timestamptz > clock_timestamp()").bind(description.expires_at).fetch_one(&mut *access.tx).await?;
        if !live {return Err(Error::Forbidden);}
        Ok(())
    }.await;
    if let Err(error) = result {
        return access.finish(Err(error)).await;
    }
    Ok((access, description))
}

pub(crate) async fn admit(
    State(f): State<Federation>,
    headers: HeaderMap,
    Json(input): Json<Input>,
) -> Result<Json<Admission>> {
    let source = crate::api::peer_node(&headers)?;
    let (mut access, description) = lease(&f, source, input.grant_id).await?;
    let result=async {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,71003209))")
            .bind(format!("{source}:{}",description.task.id)).execute(&mut *access.tx).await?;
        let legacy:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runs WHERE home_node=$1 AND task_id=$2)")
            .bind(source).bind(description.task.id).fetch_one(&mut *access.tx).await?;
        if legacy {return Err(Error::Conflict("task already has an incompatible execution".into()));}
        let proposed=Uuid::new_v4();
        sqlx::query("INSERT INTO authorization_remote_admissions(id,source_node,grant_id,task_id,tenant,credential_id,subject_chain,description) VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT DO NOTHING")
            .bind(proposed).bind(source).bind(input.grant_id).bind(description.task.id).bind(&access.identity.tenant).bind(access.identity.credential_id).bind(&access.subjects).bind(serde_json::to_value(&description)?).execute(&mut *access.tx).await?;
        let record:Record=sqlx::query_as("SELECT * FROM authorization_remote_admissions WHERE source_node=$1 AND grant_id=$2 FOR SHARE").bind(source).bind(input.grant_id).fetch_optional(&mut *access.tx).await?.ok_or_else(||Error::Conflict("source task already has a different admission".into()))?;
        if !record.matches(&access,&description)? {return Err(Error::Conflict("admission already binds different authority".into()));}
        let live:bool=sqlx::query_scalar("SELECT $1::timestamptz > clock_timestamp()").bind(description.expires_at).fetch_one(&mut *access.tx).await?;
        if !live {return Err(Error::Forbidden);}
        // Policy decisions are retained by Access. No unscoped workspace event
        // may disclose this source task to receiver tenants.
        Ok(Json(record.view(&description)))
    }.await;
    access.finish(result).await
}

pub(crate) async fn verify(
    State(f): State<Federation>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<bool>> {
    let source = crate::api::peer_node(&headers)?;
    let record: Record = sqlx::query_as(
        "SELECT * FROM authorization_remote_admissions WHERE id=$1 AND source_node=$2",
    )
    .bind(id)
    .bind(source)
    .fetch_optional(&f.store.pool)
    .await?
    .ok_or(Error::Forbidden)?;
    let (access, description) = lease(&f, source, record.grant_id).await?;
    let result = if record.matches(&access, &description)? {
        Ok(Json(true))
    } else {
        Err(Error::Forbidden)
    };
    access.finish(result).await
}
