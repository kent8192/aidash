use super::{Authorization, Snapshot};
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

/// Constructed only after authenticating a bearer token. Never deserialize this
/// from a request body, query parameter or peer-provided identity claim.
#[derive(Clone)]
pub enum Actor {
    Operator,
    Subject(SubjectIdentity),
}

#[derive(Clone)]
pub struct SubjectIdentity {
    pub(super) credential_id: Uuid,
    pub(super) tenant: String,
    pub(super) subject: String,
}

#[derive(Debug, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Credential {
    pub id: Uuid,
    pub tenant: String,
    pub subject: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub issued_by: String,
}

/// The bearer value is returned exactly once; only its SHA-256 digest is stored.
#[derive(Serialize, utoipa::ToSchema)]
pub struct IssuedCredential {
    pub credential: Credential,
    pub token: String,
}

fn digest(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

fn enabled(snapshot: &Snapshot, subject: &str) -> bool {
    if snapshot.bundle.validate().is_err() {
        return false;
    }
    let mut current = Some(subject);
    while let Some(id) = current {
        let Some(subject) = snapshot.bundle.subjects.get(id) else {
            return false;
        };
        if !subject.enabled {
            return false;
        }
        current = subject.delegated_by.as_deref();
    }
    true
}

impl Authorization {
    pub async fn issue_credential(
        &self,
        tenant: &str,
        subject: &str,
        lifetime: i64,
        actor: &str,
    ) -> Result<IssuedCredential> {
        super::policy::identifier(actor)?;
        if !(1..=2_592_000).contains(&lifetime) {
            return Err(Error::Invalid(
                "credential lifetime must be 1..2592000 seconds".into(),
            ));
        }
        let mut tx = self.pool.begin().await?;
        let snapshot = Self::load(&mut tx, tenant).await?;
        if !enabled(&snapshot, subject) {
            return Err(Error::Invalid(
                "credential subject must exist and be enabled, including its delegators".into(),
            ));
        }
        // Two independently generated UUIDv4 values supply 244 random bits.
        let token = format!(
            "aidash_subject_{}{}",
            Uuid::new_v4().simple(),
            Uuid::new_v4().simple()
        );
        let credential = sqlx::query_as("INSERT INTO authorization_credentials(id,tenant,subject,token_hash,expires_at,issued_by) VALUES($1,$2,$3,$4,clock_timestamp()+make_interval(secs => $5),$6) RETURNING id,tenant,subject,created_at,expires_at,revoked_at,issued_by")
            .bind(Uuid::new_v4()).bind(tenant).bind(subject).bind(digest(&token)).bind(lifetime as f64).bind(actor).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(IssuedCredential { credential, token })
    }

    pub async fn credentials(&self, tenant: &str) -> Result<Vec<Credential>> {
        Ok(sqlx::query_as("SELECT id,tenant,subject,created_at,expires_at,revoked_at,issued_by FROM authorization_credentials WHERE tenant=$1 ORDER BY created_at DESC,id LIMIT 200")
            .bind(tenant).fetch_all(&self.pool).await?)
    }

    pub async fn revoke_credential(&self, tenant: &str, id: Uuid) -> Result<Credential> {
        sqlx::query_as("UPDATE authorization_credentials SET revoked_at=coalesce(revoked_at,clock_timestamp()) WHERE tenant=$1 AND id=$2 RETURNING id,tenant,subject,created_at,expires_at,revoked_at,issued_by")
            .bind(tenant).bind(id).fetch_optional(&self.pool).await?
            .ok_or_else(|| Error::NotFound("credential".into()))
    }

    pub async fn authenticate(&self, token: &str) -> Result<Actor> {
        if token.len() > 256 || !token.starts_with("aidash_subject_") {
            return Err(Error::Unauthorized);
        }
        let row: Option<(Uuid, String, String)> = sqlx::query_as("SELECT id,tenant,subject FROM authorization_credentials WHERE token_hash=$1 AND revoked_at IS NULL AND expires_at>clock_timestamp()")
            .bind(digest(token)).fetch_optional(&self.pool).await?;
        let (credential_id, tenant, subject) = row.ok_or(Error::Unauthorized)?;
        Ok(Actor::Subject(SubjectIdentity {
            credential_id,
            tenant,
            subject,
        }))
    }
}

impl SubjectIdentity {
    /// Kept through the protected transaction: a concurrent credential or policy
    /// revocation must wait for this boundary, and the next boundary reloads both.
    pub(super) async fn lock(&self, tx: &mut Transaction<'_, Postgres>) -> Result<Snapshot> {
        let snapshot = Authorization::load(tx, &self.tenant).await?;
        let valid: Option<Uuid> = sqlx::query_scalar("SELECT id FROM authorization_credentials WHERE id=$1 AND tenant=$2 AND subject=$3 AND revoked_at IS NULL AND expires_at>clock_timestamp() FOR SHARE")
            .bind(self.credential_id).bind(&self.tenant).bind(&self.subject).fetch_optional(&mut **tx).await?;
        if valid.is_none() {
            return Err(Error::Unauthorized);
        }
        if !enabled(&snapshot, &self.subject) {
            return Err(Error::Forbidden);
        }
        Ok(snapshot)
    }
}
