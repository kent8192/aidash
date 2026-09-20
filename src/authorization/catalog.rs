use super::{Authorization, access::Access, identity::SubjectIdentity};
use crate::{
    Error, Result,
    registry::{EntityRef, Entry, Search},
    store::Store,
};
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Binding {
    pub tenant: String,
    pub entry_id: String,
    pub entry_version: String,
    pub enabled: bool,
    pub revision: i64,
}

impl Authorization {
    pub async fn set_catalog(
        &self,
        tenant: &str,
        entry: &EntityRef,
        expected_revision: i64,
        enabled: bool,
        actor: &str,
    ) -> Result<Binding> {
        if !(0..i64::MAX).contains(&expected_revision) {
            return Err(Error::Invalid("invalid catalog revision".into()));
        }
        let mut tx = self.pool.begin().await?;
        Self::load(&mut tx, tenant).await?;
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM registry WHERE id=$1 AND version=$2)")
                .bind(&entry.id)
                .bind(&entry.version)
                .fetch_one(&mut *tx)
                .await?;
        if !exists {
            return Err(Error::NotFound("registry entry".into()));
        }
        let binding: Option<Binding> = if expected_revision == 0 {
            sqlx::query_as("INSERT INTO authorization_catalog(tenant,entry_id,entry_version,enabled,revision) VALUES($1,$2,$3,$4,1) ON CONFLICT DO NOTHING RETURNING *")
                .bind(tenant).bind(&entry.id).bind(&entry.version).bind(enabled).fetch_optional(&mut *tx).await?
        } else {
            sqlx::query_as("UPDATE authorization_catalog SET enabled=$4,revision=revision+1 WHERE tenant=$1 AND entry_id=$2 AND entry_version=$3 AND revision=$5 RETURNING *")
                .bind(tenant).bind(&entry.id).bind(&entry.version).bind(enabled).bind(expected_revision).fetch_optional(&mut *tx).await?
        };
        let binding = binding.ok_or_else(|| Error::Conflict("catalog revision changed".into()))?;
        sqlx::query("INSERT INTO authorization_catalog_history(tenant,entry_id,entry_version,revision,enabled,actor) VALUES($1,$2,$3,$4,$5,$6)")
            .bind(tenant).bind(&entry.id).bind(&entry.version).bind(binding.revision).bind(enabled).bind(actor).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(binding)
    }

    pub async fn catalog(&self, tenant: &str) -> Result<Vec<Binding>> {
        Ok(sqlx::query_as(
            "SELECT * FROM authorization_catalog WHERE tenant=$1 ORDER BY entry_id,entry_version",
        )
        .bind(tenant)
        .fetch_all(&self.pool)
        .await?)
    }
}

pub(crate) async fn entry(
    access: &mut Access,
    reference: &EntityRef,
    action: &str,
) -> Result<Entry> {
    let key = (reference.id.clone(), reference.version.clone());
    if access.inherited_lease && !access.approved_catalog.contains(&key) {
        return Err(Error::Forbidden);
    }
    let query = if access.inherited_lease {
        "SELECT r.metadata FROM authorization_catalog c JOIN registry r ON r.id=c.entry_id AND r.version=c.entry_version WHERE c.tenant=$1 AND c.entry_id=$2 AND c.entry_version=$3 AND c.enabled"
    } else {
        "SELECT r.metadata FROM authorization_catalog c JOIN registry r ON r.id=c.entry_id AND r.version=c.entry_version WHERE c.tenant=$1 AND c.entry_id=$2 AND c.entry_version=$3 AND c.enabled FOR SHARE OF c"
    };
    let document: Option<Value> = sqlx::query_scalar(query)
        .bind(&access.identity.tenant)
        .bind(&reference.id)
        .bind(&reference.version)
        .fetch_optional(&mut *access.tx)
        .await?;
    let entry: Entry = serde_json::from_value(document.ok_or(Error::Forbidden)?)?;
    access.require(&resource(access, &entry), action).await?;
    access.approved_catalog.insert(key);
    Ok(entry)
}

pub(crate) fn resource(access: &Access, entry: &Entry) -> super::policy::Resource {
    access.resource(&entry.kind,&entry.id,json!({"version":entry.version,"capabilities":entry.capabilities,"tags":entry.tags,"languages":entry.languages,"config":entry.config}))
}

pub(crate) async fn list_in(access: &mut Access, search: &Search) -> Result<Vec<Entry>> {
    let query = if access.inherited_lease {
        "SELECT r.metadata FROM authorization_catalog c JOIN registry r ON r.id=c.entry_id AND r.version=c.entry_version WHERE c.tenant=$1 AND c.enabled ORDER BY c.entry_id,c.entry_version"
    } else {
        "SELECT r.metadata FROM authorization_catalog c JOIN registry r ON r.id=c.entry_id AND r.version=c.entry_version WHERE c.tenant=$1 AND c.enabled ORDER BY c.entry_id,c.entry_version FOR SHARE OF c"
    };
    let documents: Vec<Value> = sqlx::query_scalar(query)
        .bind(&access.identity.tenant)
        .fetch_all(&mut *access.tx)
        .await?;
    let mut entries = vec![];
    for document in documents {
        let entry: Entry = serde_json::from_value(document)?;
        if (!access.inherited_lease
            || access
                .approved_catalog
                .contains(&(entry.id.clone(), entry.version.clone())))
            && search.matches(&entry)
            && access
                .decide(&resource(access, &entry), "registry.read")
                .await?
        {
            access
                .approved_catalog
                .insert((entry.id.clone(), entry.version.clone()));
            entries.push(entry);
        }
    }
    Ok(entries)
}

pub async fn list(
    store: &Store,
    identity: &SubjectIdentity,
    search: &Search,
) -> Result<Vec<Entry>> {
    let mut access = Access::begin(store, identity).await?;
    let result = list_in(&mut access, search).await;
    access.finish(result).await
}

pub async fn get(
    store: &Store,
    identity: &SubjectIdentity,
    reference: &EntityRef,
) -> Result<Entry> {
    let mut access = Access::begin(store, identity).await?;
    let result = entry(&mut access, reference, "registry.read").await;
    access.finish(result).await
}
