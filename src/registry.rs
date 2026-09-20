use crate::{
    Error, Result,
    config::{secret, validate_endpoint},
    domain::empty_object,
};
use sea_orm::{
    ActiveValue::Set, DbBackend, QueryOrder, Statement, TransactionTrait, entity::prelude::*,
    sea_query::OnConflict,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

mod record {
    use sea_orm::entity::prelude::*;
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "registry")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub version: String,
        pub kind: String,
        pub metadata: Json,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}

pub type Localized = BTreeMap<String, String>;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub id: String,
    pub version: String,
    pub kind: String,
    pub name: Localized,
    pub description: Localized,
    #[serde(default)]
    #[schema(required = true)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    #[schema(required = true)]
    pub tags: Vec<String>,
    #[serde(default)]
    #[schema(required = true)]
    pub languages: Vec<String>,
    #[serde(default)]
    #[schema(required = true)]
    pub skills: Vec<String>,
    #[serde(default = "empty_object")]
    #[schema(value_type = BTreeMap<String, Value>, required = true)]
    pub schema: Value,
    #[serde(default = "empty_object")]
    #[schema(value_type = BTreeMap<String, Value>, required = true)]
    pub config: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
pub struct EntityRef {
    pub id: String,
    pub version: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    pub model: EntityRef,
    pub instructions: String,
    #[serde(default)]
    pub tools: Vec<EntityRef>,
    #[serde(default)]
    pub skills: Vec<EntityRef>,
    pub cluster: Option<EntityRef>,
    #[serde(default = "max_steps")]
    pub max_steps: i32,
}
fn max_steps() -> i32 {
    64
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ClusterConfig {
    pub coordinator: EntityRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelConfig {
    pub provider: String,
    pub model_id: String,
    pub endpoint: String,
    pub credential_env: Option<String>,
    pub context_window: usize,
    pub modalities: Vec<String>,
    pub cost: Value,
}

/// Explicit transport and bounds for System One probability classification.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CompactorConfig {
    pub provider: String,
    pub endpoint: String,
    pub model: String,
    pub credential_env: String,
    pub max_request_bytes: usize,
    pub max_questions: usize,
    pub max_response_bytes: usize,
}
impl CompactorConfig {
    pub fn validate(&self) -> Result<()> {
        validate_endpoint(&self.endpoint)?;
        if self.provider != "typesafe-system-one"
            || self.model.trim().is_empty()
            || self.model.len() > 128
            || !(1024..=1_048_576).contains(&self.max_request_bytes)
            || !(1..=1024).contains(&self.max_questions)
            || !(128..=1_048_576).contains(&self.max_response_bytes)
        {
            return Err(Error::Invalid(
                "invalid compactor transport or request/response bounds".into(),
            ));
        }
        if secret(&self.credential_env)?.trim().is_empty() {
            return Err(Error::Invalid(
                "compactor credential must not be empty".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[derive(utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct Search {
    pub kind: Option<String>,
    pub query: Option<String>,
    pub capability: Option<String>,
    pub language: Option<String>,
    pub skill: Option<String>,
    pub tag: Option<String>,
    pub model: Option<String>,
}
impl Search {
    pub fn matches(&self, e: &Entry) -> bool {
        self.kind.as_ref().is_none_or(|s| &e.kind == s)
            && self
                .capability
                .as_ref()
                .is_none_or(|s| e.capabilities.contains(s))
            && self
                .language
                .as_ref()
                .is_none_or(|s| e.languages.iter().any(|l| l.eq_ignore_ascii_case(s)))
            && self.skill.as_ref().is_none_or(|s| e.skills.contains(s))
            && self.tag.as_ref().is_none_or(|s| e.tags.contains(s))
            && self
                .model
                .as_ref()
                .is_none_or(|s| e.config.pointer("/model/id").and_then(Value::as_str) == Some(s))
            && self.query.as_ref().is_none_or(|s| {
                let s = s.to_lowercase();
                e.id.to_lowercase().contains(&s)
                    || e.name
                        .values()
                        .chain(e.description.values())
                        .any(|v| v.to_lowercase().contains(&s))
            })
    }
}

#[derive(Clone)]
pub struct Registry {
    pub db: DatabaseConnection,
}
impl Registry {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self {
            db: sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(pool),
        }
    }
    pub async fn get(&self, id: &str, version: &str) -> Result<Entry> {
        let row = record::Entity::find_by_id((id.to_owned(), version.to_owned()))
            .one(&self.db)
            .await?
            .ok_or_else(|| Error::NotFound(format!("entity {id}@{version}")))?;
        Ok(serde_json::from_value(row.metadata)?)
    }
    pub async fn list(&self, search: &Search) -> Result<Vec<Entry>> {
        let mut query = record::Entity::find();
        if let Some(kind) = &search.kind {
            query = query.filter(record::Column::Kind.eq(kind));
        }
        let rows = query
            .order_by_asc(record::Column::Id)
            .order_by_asc(record::Column::Version)
            .all(&self.db)
            .await?;
        let mut result = Vec::new();
        for row in rows {
            let e: Entry = serde_json::from_value(row.metadata)?;
            if search.matches(&e) {
                result.push(e);
            }
        }
        Ok(result)
    }
    pub async fn register(&self, e: Entry) -> Result<Entry> {
        self.validate_references(&e).await?;
        let tx = self.db.begin().await?;
        insert_entry(&tx, &e).await?;
        tx.commit().await?;
        Ok(e)
    }
    pub async fn validate_references(&self, e: &Entry) -> Result<()> {
        validate(e)?;
        if e.kind == "agent" {
            let cfg: AgentConfig = serde_json::from_value(e.config.clone())?;
            for (r, kind) in std::iter::once((&cfg.model, "model"))
                .chain(cfg.tools.iter().map(|r| (r, "tool")))
                .chain(cfg.skills.iter().map(|r| (r, "skill")))
                .chain(cfg.cluster.iter().map(|r| (r, "cluster")))
            {
                if self.get(&r.id, &r.version).await?.kind != kind {
                    return Err(Error::Invalid(format!("{} must reference a {kind}", r.id)));
                }
            }
        }
        if e.kind == "cluster" {
            let config: ClusterConfig = serde_json::from_value(e.config.clone())?;
            if self
                .get(&config.coordinator.id, &config.coordinator.version)
                .await?
                .kind
                != "agent"
            {
                return Err(Error::Invalid(
                    "cluster coordinator must reference an agent".into(),
                ));
            }
        }
        Ok(())
    }
}

async fn insert_entry<C: ConnectionTrait>(db: &C, e: &Entry) -> Result<()> {
    let value = serde_json::to_value(e)?;
    record::Entity::insert(record::ActiveModel {
        id: Set(e.id.clone()),
        version: Set(e.version.clone()),
        kind: Set(e.kind.clone()),
        metadata: Set(value.clone()),
    })
    .on_conflict(
        OnConflict::columns([record::Column::Id, record::Column::Version])
            .do_nothing()
            .to_owned(),
    )
    .do_nothing()
    .exec(db)
    .await?;
    let stored = record::Entity::find_by_id((e.id.clone(), e.version.clone()))
        .one(db)
        .await?
        .ok_or_else(|| Error::Conflict("entity was concurrently removed".into()))?;
    if stored.metadata != value {
        return Err(Error::Conflict(
            "published versions are immutable; choose a new version".into(),
        ));
    }
    Ok(())
}

pub fn validate(e: &Entry) -> Result<()> {
    let schema = json!({"type":"object","required":["id","version","kind","name","description","capabilities","tags","languages","schema","config"],
        "properties":{
            "id":{"type":"string","pattern":"^[a-zA-Z0-9][a-zA-Z0-9._-]{0,99}$"},
            "version":{"type":"string"}, "kind":{"enum":["agent","model","tool","skill","cluster","node","compactor"]},
            "name":{"type":"object","minProperties":1,"additionalProperties":{"type":"string","minLength":1}},
            "description":{"type":"object","minProperties":1,"additionalProperties":{"type":"string"}},
            "capabilities":{"type":"array","items":{"type":"string"},"uniqueItems":true},
            "tags":{"type":"array","items":{"type":"string"},"uniqueItems":true},
            "languages":{"type":"array","items":{"type":"string","pattern":"^[a-zA-Z]{2,8}(-[a-zA-Z0-9]{1,8})*$"},"uniqueItems":true},
            "schema":{"type":"object"},"config":{"type":"object"}}});
    let validator =
        jsonschema::validator_for(&schema).map_err(|e| Error::Invalid(e.to_string()))?;
    validator
        .validate(&serde_json::to_value(e)?)
        .map_err(|e| Error::Invalid(e.to_string()))?;
    semver::Version::parse(&e.version)
        .map_err(|_| Error::Invalid("version must be semantic versioning".into()))?;
    for locale in e.name.keys().chain(e.description.keys()) {
        if locale.is_empty()
            || !locale.split('-').all(|p| {
                !p.is_empty() && p.len() <= 8 && p.chars().all(|c| c.is_ascii_alphanumeric())
            })
        {
            return Err(Error::Invalid(
                "metadata locales must be BCP 47 language tags".into(),
            ));
        }
    }
    // Remote schema resolution is disabled: metadata must be self contained.
    jsonschema::validator_for(&e.schema)
        .map_err(|e| Error::Invalid(format!("invalid entity schema: {e}")))?;
    match e.kind.as_str() {
        "compactor" => serde_json::from_value::<CompactorConfig>(e.config.clone())
            .map_err(|e| Error::Invalid(e.to_string()))?
            .validate()?,
        "model" => {
            let m: ModelConfig = serde_json::from_value(e.config.clone())
                .map_err(|e| Error::Invalid(e.to_string()))?;
            if !matches!(m.provider.as_str(), "openai" | "anthropic" | "openrouter")
                || m.model_id.is_empty()
                || m.context_window < 2048
                || !m.modalities.iter().any(|m| m == "text")
            {
                return Err(Error::Invalid("model requires openai/anthropic/openrouter, model_id, text modality and context_window >= 2048".into()));
            }
            validate_endpoint(&m.endpoint)?;
            if let Some(name) = m.credential_env {
                secret(&name)?;
            }
        }
        "agent" => {
            let a: AgentConfig = serde_json::from_value(e.config.clone())
                .map_err(|e| Error::Invalid(e.to_string()))?;
            if a.instructions.trim().is_empty() || !(1..=1000).contains(&a.max_steps) {
                return Err(Error::Invalid(
                    "agent requires instructions and max_steps in 1..1000".into(),
                ));
            }
        }
        "cluster" => {
            let cluster: ClusterConfig =
                serde_json::from_value(e.config.clone()).map_err(|error| {
                    Error::Invalid(format!("cluster requires a coordinator reference: {error}"))
                })?;
            if cluster.coordinator.id.trim().is_empty()
                || semver::Version::parse(&cluster.coordinator.version).is_err()
            {
                return Err(Error::Invalid(
                    "cluster coordinator requires an id and semantic version".into(),
                ));
            }
        }
        "tool" => crate::tool::validate_config(&e.config)?,
        "skill"
            if e.config
                .get("instructions")
                .and_then(Value::as_str)
                .is_none_or(|s| s.is_empty()) =>
        {
            return Err(Error::Invalid("skill requires instructions".into()));
        }
        _ => {}
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Package {
    pub entity: Entry,
    pub author: String,
    pub permissions: Vec<String>,
    #[serde(default)]
    #[schema(required = true)]
    pub dependencies: Vec<EntityRef>,
}
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct PackageRecord {
    pub id: String,
    pub version: String,
    #[schema(value_type = Package)]
    pub manifest: Value,
    pub digest: String,
}

pub fn digest(value: &Value) -> String {
    format!("sha256:{:x}", Sha256::digest(value.to_string().as_bytes()))
}

impl Registry {
    pub async fn publish(&self, pool: &sqlx::PgPool, package: Package) -> Result<PackageRecord> {
        validate(&package.entity)?;
        if !matches!(package.entity.kind.as_str(), "agent" | "tool" | "skill")
            || package.author.trim().is_empty()
        {
            return Err(Error::Invalid(
                "packages require an author and an agent, tool or skill".into(),
            ));
        }
        let value = serde_json::to_value(&package)?;
        let hash = digest(&value);
        sqlx::query("INSERT INTO packages(id,version,manifest,digest) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING")
            .bind(&package.entity.id).bind(&package.entity.version).bind(&value).bind(&hash).execute(pool).await?;
        let record =
            sqlx::query_as::<_, PackageRecord>("SELECT * FROM packages WHERE id=$1 AND version=$2")
                .bind(&package.entity.id)
                .bind(&package.entity.version)
                .fetch_one(pool)
                .await?;
        if record.digest != hash {
            return Err(Error::Conflict("package version is immutable".into()));
        }
        Ok(record)
    }
    pub async fn install(
        &self,
        pool: &sqlx::PgPool,
        id: &str,
        version: &str,
        expected_digest: &str,
        config: Value,
    ) -> Result<Entry> {
        let record =
            sqlx::query_as::<_, PackageRecord>("SELECT * FROM packages WHERE id=$1 AND version=$2")
                .bind(id)
                .bind(version)
                .fetch_optional(pool)
                .await?
                .ok_or_else(|| Error::NotFound("package".into()))?;
        if record.digest != expected_digest || digest(&record.manifest) != expected_digest {
            return Err(Error::Conflict("package digest changed".into()));
        }
        let package: Package = serde_json::from_value(record.manifest)?;
        let mut effective = package.entity.clone();
        overlay_config(&mut effective.config, &config)?;
        self.validate_references(&effective).await?;
        for dep in &package.dependencies {
            self.get(&dep.id, &dep.version).await?;
        }
        // Registry registration and local installation configuration are atomic.
        let tx = self.db.begin().await?;
        insert_entry(&tx, &effective).await?;
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,
            "INSERT INTO installations(id,version,digest,config) VALUES($1,$2,$3,$4) ON CONFLICT(id,version) DO UPDATE SET config=EXCLUDED.config",
            [id.into(), version.into(), expected_digest.into(), config.into()])).await?;
        tx.commit().await?;
        Ok(effective)
    }
}

/// Transactional registration for compound admission. References and metadata
/// use the same immutable version contract as the public Registry operation.
pub(crate) async fn register_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    entry: &Entry,
) -> Result<()> {
    validate(entry)?;
    if entry.kind == "agent" {
        let config: AgentConfig = serde_json::from_value(entry.config.clone())?;
        for (reference, kind) in std::iter::once((&config.model, "model"))
            .chain(config.tools.iter().map(|r| (r, "tool")))
            .chain(config.skills.iter().map(|r| (r, "skill")))
            .chain(config.cluster.iter().map(|r| (r, "cluster")))
        {
            let actual: Option<String> =
                sqlx::query_scalar("SELECT kind FROM registry WHERE id=$1 AND version=$2")
                    .bind(&reference.id)
                    .bind(&reference.version)
                    .fetch_optional(&mut **tx)
                    .await?;
            if actual.as_deref() != Some(kind) {
                return Err(Error::Invalid(format!(
                    "{} must reference a {kind}",
                    reference.id
                )));
            }
        }
    }
    if entry.kind == "cluster" {
        let config: ClusterConfig = serde_json::from_value(entry.config.clone())?;
        let kind: Option<String> =
            sqlx::query_scalar("SELECT kind FROM registry WHERE id=$1 AND version=$2")
                .bind(config.coordinator.id)
                .bind(config.coordinator.version)
                .fetch_optional(&mut **tx)
                .await?;
        if kind.as_deref() != Some("agent") {
            return Err(Error::Invalid(
                "cluster coordinator must reference an agent".into(),
            ));
        }
    }
    let value = serde_json::to_value(entry)?;
    sqlx::query(
        "INSERT INTO registry(id,version,kind,metadata) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING",
    )
    .bind(&entry.id)
    .bind(&entry.version)
    .bind(&entry.kind)
    .bind(&value)
    .execute(&mut **tx)
    .await?;
    let stored: Value =
        sqlx::query_scalar("SELECT metadata FROM registry WHERE id=$1 AND version=$2")
            .bind(&entry.id)
            .bind(&entry.version)
            .fetch_one(&mut **tx)
            .await?;
    if stored != value {
        return Err(Error::Conflict(
            "published versions are immutable; choose a new version".into(),
        ));
    }
    Ok(())
}

fn overlay_config(target: &mut Value, overrides: &Value) -> Result<()> {
    let object = overrides
        .as_object()
        .ok_or_else(|| Error::Invalid("installation config must be an object".into()))?;
    let target = target
        .as_object_mut()
        .ok_or_else(|| Error::Invalid("entity config must be an object".into()))?;
    for (key, value) in object {
        target.insert(key.clone(), value.clone());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry() -> Entry {
        serde_json::from_value(json!({"id":"research","version":"1.0.0","kind":"skill","name":{"en":"Research","ja":"調査"},"description":{"en":"Research"},"capabilities":["web.search"],"languages":["ja","en"],"config":{"instructions":"Research carefully"}})).unwrap()
    }
    #[test]
    fn conjunctive_search_and_localization() {
        let e = entry();
        validate(&e).unwrap();
        assert!(
            Search {
                capability: Some("web.search".into()),
                language: Some("JA".into()),
                ..Default::default()
            }
            .matches(&e)
        );
        assert!(
            !Search {
                capability: Some("web.search".into()),
                language: Some("fr".into()),
                ..Default::default()
            }
            .matches(&e)
        );
        assert!(
            Search {
                query: Some("調査".into()),
                ..Default::default()
            }
            .matches(&e)
        );
    }
    #[test]
    fn rejects_invalid_metadata() {
        let mut e = entry();
        e.version = "latest".into();
        assert!(validate(&e).is_err());
        e.version = "1.0.0".into();
        e.id = "../../escape".into();
        assert!(validate(&e).is_err());
    }
    #[test]
    fn unknown_requirements_and_invalid_clusters_are_rejected() {
        for value in [
            json!({"capabilty":"web.search"}),
            json!({"capabilities":["web.search"]}),
        ] {
            assert!(serde_json::from_value::<Search>(value).is_err());
        }
        let mut e = entry();
        e.kind = "cluster".into();
        for config in [
            json!({}),
            json!({"coordinator":"research"}),
            json!({"coordinator":{"id":"research","version":"latest"}}),
        ] {
            e.config = config;
            assert!(validate(&e).is_err());
        }
        e.config = json!({"coordinator":{"id":"research","version":"1.0.0"}});
        validate(&e).unwrap();
    }
}
