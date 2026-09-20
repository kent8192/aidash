//! Every provider attempt reserves a durable call against all generated ancestors.
//! The outer worker authority lease remains held through the HTTP request.
use crate::{
    Error, Result,
    authorization::{access::Access, catalog},
    context::jev::{JevAsker, JevClient, Questions},
    domain::Run,
    registry::{CompactorConfig, EntityRef},
    store::Store,
};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

pub(crate) struct ApprovedCompactor {
    pub access: Arc<Mutex<Access>>,
    pub store: Store,
    pub run: Run,
    pub client: reqwest::Client,
}

#[async_trait::async_trait]
impl JevAsker for ApprovedCompactor {
    async fn ask(&self, state: &Value, questions: &Questions) -> Result<Value> {
        let mut access = self.access.lock().await;
        let jobs: Vec<(Uuid, Value)> = sqlx::query_as("SELECT r.id,h.spec FROM generation_requests r JOIN generation_policy_history h ON h.tenant=r.tenant AND h.policy_id=r.policy_id AND h.revision=r.policy_revision WHERE r.tenant=$1 AND ($2 || '/agents/' || r.agent_id || '@' || r.agent_version)=ANY($3) ORDER BY r.id")
            .bind(access.identity.tenant.clone()).bind(&self.store.node_id).bind(access.subjects.clone()).fetch_all(&mut *access.tx).await?;
        if jobs.is_empty() {
            drop(access);
            return JevClient::from_env(self.client.clone())?
                .ask(state, questions)
                .await;
        }
        super::provision::require_live(
            &mut access,
            &self.store.node_id,
            self.run.task_id,
            &EntityRef {
                id: self.run.agent_id.clone(),
                version: self.run.agent_version.clone(),
            },
        )
        .await?;
        let mut reference = None;
        for (_, document) in &jobs {
            let spec: super::policy::Spec = serde_json::from_value(document.clone())?;
            let approved = spec.compaction.ok_or_else(|| {
                Error::Invalid(
                    "generated context requires a separately approved compaction provider".into(),
                )
            })?;
            if reference.as_ref().is_some_and(|r| r != &approved.provider) {
                return Err(Error::Forbidden);
            }
            reference = Some(approved.provider);
        }
        let reference = reference.ok_or(Error::Forbidden)?;
        catalog::entry(&mut access, &reference, "registry.read").await?;
        let entry = catalog::entry(&mut access, &reference, "compaction.invoke").await?;
        if entry.kind != "compactor" {
            return Err(Error::Forbidden);
        }
        let config: CompactorConfig = serde_json::from_value(entry.config)?;
        let transport = JevClient::approved(config)?;
        let bytes = transport.check_request(state, questions)?;
        let attempt = Uuid::new_v4();
        let mut tx = self.store.pool.begin().await?;
        // Stable order matches inference reservations. The whole chain commits
        // or rolls back together; concurrent batches cannot overspend one slot.
        for (id, _) in jobs {
            let reserved: Option<Uuid> = sqlx::query_scalar("UPDATE generation_budgets SET compaction_calls=compaction_calls+1 WHERE request_id=$1 AND compaction_calls<compaction_call_limit RETURNING request_id")
                .bind(id).fetch_optional(&mut *tx).await?;
            if reserved.is_none() {
                return Err(Error::Invalid(
                    "generated compaction call budget exhausted".into(),
                ));
            }
            sqlx::query("INSERT INTO generation_compaction_usage(request_id,attempt_id,run_id,provider_id,provider_version,request_bytes,questions) VALUES($1,$2,$3,$4,$5,$6,$7)")
                .bind(id).bind(attempt).bind(self.run.id).bind(&reference.id).bind(&reference.version).bind(bytes as i64).bind(questions.len() as i32).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        drop(access);
        // Failed, uncertain and crashed requests stay charged. Retrying always
        // reserves a fresh attempt, including retries of the same worker step.
        transport.ask(state, questions).await
    }
}
