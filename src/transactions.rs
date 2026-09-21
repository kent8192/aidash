pub mod api;
pub mod coordinator;
pub mod gate;
mod mutation;
pub mod participant;

use crate::{Error, Result, config::validate_node_id, domain::ArtifactInput, registry::Entry};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as=TransactionIsolation)]
pub enum Isolation {
    Serializable,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[schema(as=TransactionMutation)]
pub enum Mutation {
    RegistryRegister {
        entry: Box<Entry>,
    },
    WorkspaceState {
        workspace_id: Uuid,
        expected_revision: i64,
        state: Value,
    },
    CompleteTask {
        task_id: Uuid,
        expected_revision: i64,
        artifact: ArtifactInput,
    },
    FinishRun {
        run_id: Uuid,
        task_id: Uuid,
        expected_revision: i64,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as=TransactionParticipant)]
pub struct Participant {
    pub node_id: String,
    pub mutations: Vec<Mutation>,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as=TransactionManifest)]
pub struct Manifest {
    pub id: Uuid,
    pub coordinator: String,
    pub isolation: Isolation,
    pub deadline: DateTime<Utc>,
    pub participants: Vec<Participant>,
}
impl Manifest {
    pub fn validate(&self) -> Result<()> {
        validate_node_id(&self.coordinator)?;
        if self.id.is_nil() || self.participants.is_empty() || self.participants.len() > 16 {
            return Err(Error::Invalid(
                "transaction requires an ID and 1..16 participants".into(),
            ));
        }
        let mut previous: Option<&str> = None;
        let mut count = 0;
        for participant in &self.participants {
            validate_node_id(&participant.node_id)?;
            if previous.is_some_and(|p| p >= participant.node_id.as_str())
                || participant.mutations.len() > 64
            {
                return Err(Error::Invalid("participants must be unique, sorted by node ID, with at most 64 mutations each".into()));
            }
            previous = Some(&participant.node_id);
            let mut keys = std::collections::BTreeSet::new();
            for mutation in &participant.mutations {
                let key = match mutation {
                    Mutation::RegistryRegister { entry } => {
                        crate::registry::validate_structure(entry)?;
                        format!("registry:{}@{}", entry.id, entry.version)
                    }
                    Mutation::WorkspaceState {
                        workspace_id,
                        expected_revision,
                        state,
                    } => {
                        if *expected_revision < 0 || !state.is_object() {
                            return Err(Error::Invalid(
                                "workspace mutation requires a revision and object state".into(),
                            ));
                        }
                        format!("workspace:{workspace_id}")
                    }
                    Mutation::CompleteTask {
                        task_id,
                        expected_revision,
                        artifact,
                    } => {
                        if *expected_revision < 0 {
                            return Err(Error::Invalid("task revision must be nonnegative".into()));
                        }
                        artifact.validate()?;
                        format!("task:{task_id}")
                    }
                    Mutation::FinishRun {
                        run_id,
                        expected_revision,
                        ..
                    } => {
                        if *expected_revision < 0 {
                            return Err(Error::Invalid("run revision must be nonnegative".into()));
                        }
                        format!("run:{run_id}")
                    }
                };
                if !keys.insert(key) {
                    return Err(Error::Invalid(
                        "a resource can be mutated only once per manifest".into(),
                    ));
                }
            }
            count += participant.mutations.len();
        }
        if count == 0
            || !self
                .participants
                .iter()
                .any(|p| p.node_id == self.coordinator)
            || serde_json::to_vec(self)?.len() > 524_288
        {
            return Err(Error::Invalid("transaction needs mutations, its coordinator as a participant, and a manifest within 512 KiB".into()));
        }
        Ok(())
    }
    pub fn digest(&self) -> Result<String> {
        Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(self)?)))
    }
    pub(crate) fn local(&self, node: &str) -> Result<&Participant> {
        self.participants
            .iter()
            .find(|p| p.node_id == node)
            .ok_or(Error::Forbidden)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
#[schema(as=AtomicTransaction)]
pub struct Status {
    pub id: Uuid,
    pub digest: String,
    #[schema(value_type=Manifest)]
    pub manifest: Value,
    pub decision: Option<String>,
    pub visible: bool,
    pub complete: bool,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
#[schema(as=TransactionVote)]
pub struct Vote {
    pub node_id: String,
    pub phase: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
#[schema(as=TransactionParticipantStatus)]
pub struct LocalStatus {
    pub id: Uuid,
    pub coordinator: String,
    pub digest: String,
    #[schema(value_type=Manifest)]
    pub manifest: Value,
    pub phase: String,
    pub updated_at: DateTime<Utc>,
}
pub(crate) async fn history(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
    role: &str,
    phase: &str,
    detail: &str,
) -> Result<()> {
    sqlx::query(
        &sea_orm::sea_query::Query::insert()
            .into_table(sea_orm::sea_query::Alias::new("atomic_history"))
            .columns([
                sea_orm::sea_query::Alias::new("transaction_id"),
                sea_orm::sea_query::Alias::new("role"),
                sea_orm::sea_query::Alias::new("phase"),
                sea_orm::sea_query::Alias::new("detail"),
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
    .bind(role)
    .bind(phase)
    .bind(detail)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
