use super::{Request, lifecycle, policy};
use crate::{
    Error, Result,
    authorization::{
        Authorization,
        access::Access,
        catalog, execution,
        identity::SubjectIdentity,
        policy::{Subject, SubjectKind},
    },
    domain::qualified_agent,
    federation::Federation,
    registry::{EntityRef, Entry},
};
use chrono::Utc;
use serde_json::{Value, json};
use uuid::Uuid;

async fn activate(f: &Federation, job: &Request) -> Result<()> {
    let identity = SubjectIdentity {
        credential_id: job.credential_id,
        tenant: job.tenant.clone(),
        subject: job.root_subject.clone(),
    };
    let mut access = Access::begin_exclusive(&f.store, &identity).await?;
    let result=async {
        let job=lifecycle::load(&mut access.tx,&job.tenant,job.id).await?;
        if job.status!="QUEUED" {return Ok(());}
        if job.expires_at<=Utc::now() {return Err(Error::Conflict("generation request expired".into()));}
        access.subjects=job.subject_chain.clone();
        if !job.visible(&mut access).await? {return Err(Error::Forbidden);}
        access.require(&super::resource(&access,&job.policy_id),"generation.request").await?;
        let current=policy::load(&mut access.tx,&job.tenant,&job.policy_id,true).await?;
        if !current.spec.enabled {return Err(Error::Forbidden);}
        let document:Value=sqlx::query_scalar("SELECT spec FROM generation_policy_history WHERE tenant=$1 AND policy_id=$2 AND revision=$3")
            .bind(&job.tenant).bind(&job.policy_id).bind(job.policy_revision).fetch_one(&mut *access.tx).await?;
        let spec:policy::Spec=serde_json::from_value(document)?;
        let config=spec.validate(&access.snapshot.bundle)?;
        for (reference,action) in std::iter::once((&config.model,"model.infer"))
            .chain(config.tools.iter().map(|r|(r,"tool.invoke")))
            .chain(config.skills.iter().map(|r|(r,"skill.use")))
            .chain(config.cluster.iter().map(|r|(r,"cluster.execute"))) {
            catalog::entry(&mut access,reference,"registry.read").await?;
            catalog::entry(&mut access,reference,action).await?;
        }
        let entry:Entry=serde_json::from_value(job.definition.clone())?;
        let subject=qualified_agent(&f.config.node_id,&entry.id,&entry.version);
        if access.snapshot.bundle.subjects.contains_key(&subject) {return Err(Error::Conflict("generated subject already exists".into()));}
        access.snapshot.bundle.subjects.insert(subject,Subject{
            kind:SubjectKind::Agent,roles:spec.permissions.roles,groups:spec.permissions.groups,attributes:spec.permissions.attributes,enabled:true,delegated_by:job.subject_chain.last().cloned(),
        });
        access.snapshot.bundle.validate()?;
        access.snapshot.revision=access.snapshot.revision.checked_add(1).ok_or_else(||Error::Invalid("authorization revision exhausted".into()))?;
        sqlx::query("UPDATE authorization_bundles SET revision=$2,document=$3,updated_at=now() WHERE tenant=$1")
            .bind(&job.tenant).bind(access.snapshot.revision).bind(json!(access.snapshot.bundle)).execute(&mut *access.tx).await?;
        sqlx::query("INSERT INTO authorization_revisions(tenant,revision,document,actor) VALUES($1,$2,$3,$4)")
            .bind(&job.tenant).bind(access.snapshot.revision).bind(json!(access.snapshot.bundle)).bind(&job.root_subject).execute(&mut *access.tx).await?;
        crate::registry::register_in(&mut access.tx,&entry).await?;
        sqlx::query("INSERT INTO authorization_catalog(tenant,entry_id,entry_version,enabled,revision) VALUES($1,$2,$3,true,1)")
            .bind(&job.tenant).bind(&entry.id).bind(&entry.version).execute(&mut *access.tx).await?;
        sqlx::query("INSERT INTO authorization_catalog_history(tenant,entry_id,entry_version,revision,enabled,actor) VALUES($1,$2,$3,1,true,$4)")
            .bind(&job.tenant).bind(&entry.id).bind(&entry.version).bind(&job.root_subject).execute(&mut *access.tx).await?;
        lifecycle::transition(f,&mut access.tx,&job,"ACTIVE","generation-service","registered approved definition").await?;
        execution::delegate_in(f,&mut access,job.task_id,&EntityRef{id:entry.id,version:entry.version}).await?;
        Ok(())
    }.await;
    // A failed admission rolls back the proposed policy revision too; decisions
    // referring to that uncommitted revision cannot be retained independently.
    // The reconciler records the failure in the durable generation history.
    let result = result.map_err(|e| {
        if matches!(e, Error::Forbidden) {
            Error::Conflict("generated agent admission denied".into())
        } else {
            e
        }
    });
    access.finish(result).await
}

async fn terminal(f: &Federation, job: &Request, status: &str, reason: &str) -> Result<()> {
    let mut tx = f.store.pool.begin().await?;
    Authorization::load_with_mode(&mut tx, &job.tenant, true).await?;
    let job = lifecycle::load(&mut tx, &job.tenant, job.id).await?;
    if matches!(
        job.status.as_str(),
        "PENDING_APPROVAL" | "QUEUED" | "ACTIVE"
    ) {
        // Re-read after acquiring the exclusive lease: another reconciler can
        // finish activation or completion while this one is waiting.
        let phase: Option<String> = sqlx::query_scalar("SELECT phase FROM runs WHERE task_id=$1")
            .bind(job.task_id)
            .fetch_optional(&mut *tx)
            .await?;
        let terminal_phase = match phase.as_deref() {
            Some("COMPLETED") => Some("COMPLETED"),
            Some("FAILED") => Some("FAILED"),
            Some("CANCELLED") => Some("STOPPED"),
            _ => None,
        };
        let (status, reason) = if let Some(status) = terminal_phase {
            (status, "run reached terminal state")
        } else {
            (status, reason)
        };
        lifecycle::transition(f, &mut tx, &job, status, "generation-service", reason).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Resume durable generation work after restart; bounded scans are safe with
/// concurrent provisioners because each transition rechecks state under locks.
pub async fn reconcile(f: &Federation) -> Result<usize> {
    let jobs:Vec<Request>=sqlx::query_as("SELECT g.* FROM generation_requests g LEFT JOIN runs r ON r.task_id=g.task_id WHERE g.status IN ('PENDING_APPROVAL','QUEUED','ACTIVE') AND (g.status='QUEUED' OR g.expires_at<=clock_timestamp() OR r.phase IN ('COMPLETED','FAILED','CANCELLED')) ORDER BY g.created_at,g.id LIMIT 32")
        .fetch_all(&f.store.pool).await?;
    let count = jobs.len();
    for job in jobs {
        if job.expires_at <= Utc::now() {
            terminal(f, &job, "EXPIRED", "generation lifetime elapsed").await?;
        } else if job.status == "QUEUED" {
            if let Err(error) = activate(f, &job).await {
                if matches!(
                    error,
                    Error::Unauthorized | Error::Forbidden | Error::Invalid(_) | Error::Conflict(_)
                ) {
                    terminal(f, &job, "FAILED", &error.to_string()).await?;
                } else {
                    return Err(error);
                }
            }
        } else {
            terminal(f, &job, "FAILED", "run reached terminal state").await?;
        }
    }
    if count > 0 {
        f.notify.notify_waiters();
    }
    Ok(count)
}

/// Uses the tenant policy lease already held by Access. All lifecycle and
/// policy mutations require its exclusive counterpart, avoiding lock upgrades
/// and allowing nested generation to reserve quota under a worker lease.
pub(crate) async fn require_live(
    access: &mut Access,
    node: &str,
    task: Uuid,
    agent: &EntityRef,
) -> Result<()> {
    let jobs:Vec<Request>=sqlx::query_as("SELECT * FROM generation_requests WHERE (tenant=$1 AND ($2 || '/agents/' || agent_id || '@' || agent_version)=ANY($3)) OR (agent_id=$4 AND agent_version=$5) ORDER BY id")
        .bind(&access.identity.tenant).bind(node).bind(&access.subjects).bind(&agent.id).bind(&agent.version).fetch_all(&mut *access.tx).await?;
    for job in jobs {
        let enabled: bool = sqlx::query_scalar(
            "SELECT (spec->>'enabled')::boolean FROM generation_policies WHERE tenant=$1 AND id=$2",
        )
        .bind(&job.tenant)
        .bind(&job.policy_id)
        .fetch_one(&mut *access.tx)
        .await?;
        if job.tenant != access.identity.tenant
            || job.status != "ACTIVE"
            || job.expires_at <= Utc::now()
            || !enabled
            || (job.agent_id == agent.id
                && job.agent_version == agent.version
                && job.task_id != task)
        {
            return Err(Error::Forbidden);
        }
    }
    Ok(())
}

pub async fn run(f: Federation) -> Result<()> {
    loop {
        if let Err(error) = reconcile(&f).await {
            tracing::warn!(%error,"generation reconciliation failed");
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}
