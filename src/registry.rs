use crate::{
	Error, Result,
	config::{secret, validate_endpoint},
	domain::empty_object,
};
use sea_orm::{
	ActiveValue::Set, DbBackend, QueryOrder, QuerySelect, TransactionTrait, entity::prelude::*,
	sea_query::OnConflict,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, time::Duration};

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillFile {
	pub path: String,
	pub content: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub encoding: Option<String>,
}

impl SkillFile {
	fn byte_len(&self) -> Result<usize> {
		if self.content.len() > 350_000 {
			return Err(Error::Invalid(format!(
				"{} exceeds the file size limit",
				self.path
			)));
		}
		match self.encoding.as_deref() {
			None | Some("utf8") => Ok(self.content.len()),
			Some("base64") => {
				use base64::Engine;
				base64::engine::general_purpose::STANDARD
					.decode(&self.content)
					.map(|bytes| bytes.len())
					.map_err(|_| {
						Error::Invalid(format!("{} has invalid base64 content", self.path))
					})
			}
			_ => Err(Error::Invalid(format!(
				"{} has an unsupported encoding",
				self.path
			))),
		}
	}
}

pub fn skill_files(entry: &Entry) -> Result<Vec<SkillFile>> {
	let files: Vec<SkillFile> = serde_json::from_value(
		entry
			.config
			.get("files")
			.cloned()
			.unwrap_or_else(|| json!([])),
	)
	.map_err(|error| Error::Invalid(format!("invalid skill files: {error}")))?;
	Ok(files)
}

pub(crate) fn valid_skill_file_path(path: &str) -> bool {
	!path.is_empty()
		&& path.len() <= 240
		&& !path.contains('\\')
		&& !path
			.split('/')
			.any(|part| part.is_empty() || part == "." || part == ".." || part.starts_with('.'))
		&& !path.chars().any(char::is_control)
}

pub fn skill_instructions(entry: &Entry) -> Result<String> {
	let mut instructions = entry.config["instructions"]
		.as_str()
		.ok_or_else(|| Error::Invalid("skill requires instructions".into()))?
		.to_owned();
	let files = skill_files(entry)?;
	if !files.is_empty() {
		instructions.push_str("\n\nRegistered Skill files are available through skill_read. Read a listed path only when needed:\n");
		for file in files {
			if file.encoding.as_deref() == Some("base64") {
				instructions.push_str(&format!("- {} (binary, base64 encoded)\n", file.path));
			} else {
				instructions.push_str(&format!("- {}\n", file.path));
			}
		}
	}
	Ok(instructions)
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
	pub model: EntityRef,
	#[serde(default)]
	pub instructions: String,
	/// Digest of private, node-local reference documents; never embeds their contents.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub knowledge_digest: Option<String>,
	#[serde(default)]
	pub tools: Vec<EntityRef>,
	#[serde(default)]
	pub skills: Vec<EntityRef>,
	pub cluster: Option<EntityRef>,
	#[serde(default = "max_steps")]
	pub max_steps: i32,
	/// Missing fields preserve the behavior of previously registered versions.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub allow_task_creation: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub allow_task_delegation: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub allow_memory_write: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub allow_workspace_retrieval: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub allow_cross_conversation_memory: Option<bool>,
}
impl AgentConfig {
	pub fn permits_builtin(&self, name: &str) -> bool {
		match name {
			"task_create" | "task_assign" => self.allow_task_creation != Some(false),
			"task_delegate" => self.allow_task_delegation != Some(false),
			"memory_write" => self.allow_memory_write != Some(false),
			"workspace_read" | "workspace_observe" | "workspace_wait" => {
				self.allow_workspace_retrieval != Some(false)
			}
			_ => true,
		}
	}
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
	/// Total inference request timeout in seconds, including the response body.
	/// Omitted or null values use 900 seconds; configured values must be positive.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	#[schema(minimum = 1, default = 900)]
	pub request_timeout_secs: Option<u32>,
	#[serde(default)]
	pub reasoning_effort: Option<ReasoningEffort>,
	pub context_window: usize,
	/// Maximum completion tokens reported by the selected provider model.
	/// Missing values are accepted only while reading legacy model versions.
	#[serde(default)]
	#[schema(required = true)]
	pub max_output_tokens: Option<u32>,
	pub modalities: Vec<String>,
	pub cost: Value,
}

impl ModelConfig {
	/// Resolve the provider's inference deadline without inheriting the shared
	/// HTTP client's shorter default. Validate at registration and before use.
	pub fn request_timeout(&self) -> Result<Duration> {
		let seconds = self.request_timeout_secs.unwrap_or(900);
		if seconds == 0 {
			return Err(Error::Invalid(
				"model request_timeout_secs must be greater than zero".into(),
			));
		}
		Ok(Duration::from_secs(u64::from(seconds)))
	}

	/// Preserve the historical output allowance for model versions registered
	/// before their provider limit was captured in the immutable config.
	pub fn output_token_limit(&self) -> u32 {
		self.max_output_tokens
			.unwrap_or_else(|| (self.context_window / 8).clamp(256, 4096) as u32)
	}
}

/// OpenRouter's normalized reasoning levels; omission retains the model default.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
	None,
	Minimal,
	Low,
	Medium,
	High,
	Xhigh,
	Max,
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
		self.validate_in(true)
	}
	pub(crate) fn validate_in(&self, local: bool) -> Result<()> {
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
		crate::config::validate_secret_reference(&self.credential_env)?;
		if local && secret(&self.credential_env)?.trim().is_empty() {
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
	node_id: String,
}
impl Registry {
	pub fn new(pool: sqlx::PgPool, node_id: &str) -> Self {
		Self {
			db: sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(pool),
			node_id: node_id.into(),
		}
	}
	pub async fn get(&self, id: &str, version: &str) -> Result<Entry> {
		let row = record::Entity::find_by_id((id.to_owned(), version.to_owned()))
			.one(&self.db)
			.await?
			.ok_or_else(|| Error::NotFound(format!("entity {id}@{version}")))?;
		self.effective(serde_json::from_value(row.metadata)?).await
	}
	async fn effective(&self, mut entry: Entry) -> Result<Entry> {
		use sea_orm::sea_query::{Alias, Expr, Query};
		let query = Query::select()
			.column(Alias::new("config"))
			.from(Alias::new("installations"))
			.and_where(Expr::col(Alias::new("id")).eq(&entry.id))
			.and_where(Expr::col(Alias::new("version")).eq(&entry.version))
			.to_owned();
		if let Some(row) = self.db.query_one(DbBackend::Postgres.build(&query)).await? {
			overlay_config(&mut entry.config, &row.try_get::<Value>("", "config")?)?;
		}
		Ok(entry)
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
			let e = self
				.effective(serde_json::from_value(row.metadata)?)
				.await?;
			if search.matches(&e) {
				result.push(e);
			}
		}
		Ok(result)
	}
	pub async fn legacy_agents(&self, search: &Search, offset: u64) -> Result<AgentPage> {
		use sea_orm::sea_query::{Alias, Expr, Query};
		let mut cursor = offset;
		let mut entries = vec![];
		let mut bytes = 0;
		loop {
			let rows = record::Entity::find()
				.filter(record::Column::Kind.eq("agent"))
				.order_by_asc(record::Column::Id)
				.order_by_asc(record::Column::Version)
				.limit(64)
				.offset(cursor)
				.all(&self.db)
				.await?;
			let exhausted = rows.len() < 64;
			for row in rows {
				let generated = Query::select()
					.column(Alias::new("id"))
					.from(Alias::new("generation_requests"))
					.and_where(Expr::col(Alias::new("agent_id")).eq(&row.id))
					.and_where(Expr::col(Alias::new("agent_version")).eq(&row.version))
					.limit(1)
					.to_owned();
				if self
					.db
					.query_one(DbBackend::Postgres.build(&generated))
					.await?
					.is_some()
				{
					cursor += 1;
					continue;
				}
				let entry = self
					.effective(serde_json::from_value(row.metadata)?)
					.await?;
				if !search.matches(&entry) {
					cursor += 1;
					continue;
				}
				let size = serde_json::to_vec(&entry)?.len() + 1;
				if bytes + size > 3_000_000 {
					if entries.is_empty() {
						return Err(Error::Invalid(
							"agent metadata exceeds discovery page limit".into(),
						));
					}
					return Ok(AgentPage {
						entries,
						next_offset: Some(cursor),
					});
				}
				bytes += size;
				cursor += 1;
				entries.push(entry);
				if entries.len() == 64 {
					return Ok(AgentPage {
						entries,
						next_offset: Some(cursor),
					});
				}
			}
			if exhausted {
				return Ok(AgentPage {
					entries,
					next_offset: None,
				});
			}
		}
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
			let mut references = Vec::new();
			for (r, kind) in std::iter::once((&cfg.model, "model"))
				.chain(cfg.tools.iter().map(|r| (r, "tool")))
				.chain(cfg.skills.iter().map(|r| (r, "skill")))
				.chain(cfg.cluster.iter().map(|r| (r, "cluster")))
			{
				let referenced = self.get(&r.id, &r.version).await?;
				if referenced.kind != kind {
					return Err(Error::Invalid(format!("{} must reference a {kind}", r.id)));
				}
				references.push(referenced);
			}
			validate_agent_prompt(&cfg, &references, &Value::Null)?;
		}
		if e.kind == "tool"
			&& let crate::tool::ToolConfig::Agent { node_id, agent } =
				serde_json::from_value(e.config.clone())?
			&& node_id == self.node_id
			&& self.get(&agent.id, &agent.version).await?.kind != "agent"
		{
			return Err(Error::Invalid(
				"agent tool executor must reference an agent".into(),
			));
		}
		if e.kind == "cluster" {
			let config: ClusterConfig = serde_json::from_value(e.config.clone())?;
			if self
				.get(&config.coordinator.id, &config.coordinator.version)
				.await?
				.kind != "agent"
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
	validate_in(e, true)
}
pub(crate) fn validate_structure(e: &Entry) -> Result<()> {
	validate_in(e, false)
}
fn validate_in(e: &Entry, local: bool) -> Result<()> {
	let schema = json!({"type":"object","required":["id","version","kind","name","description","capabilities","tags","languages","schema","config"],
        "properties":{
            "id":{"type":"string","pattern":"^[a-zA-Z0-9][a-zA-Z0-9._-]{0,99}$"},
            "version":{"type":"string"}, "kind":{"enum":["agent","model","tool","skill","cluster","node","compactor","embedding"]},
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
	// Reserve the largest valid node ID (108 bytes), `/agents/`, and `@`.
	// Every accepted agent must fit the 256-byte authorization subject limit.
	if e.kind == "agent" && e.id.len() + e.version.len() > 139 {
		return Err(Error::Invalid(
			"qualified agent identity exceeds 256 bytes".into(),
		));
	}
	for locale in e.name.keys().chain(e.description.keys()) {
		if locale.is_empty()
			|| !locale.split('-').all(|p| {
				!p.is_empty() && p.len() <= 8 && p.chars().all(|c| c.is_ascii_alphanumeric())
			}) {
			return Err(Error::Invalid(
				"metadata locales must be BCP 47 language tags".into(),
			));
		}
	}
	// Remote schema resolution is disabled: metadata must be self contained.
	jsonschema::validator_for(&e.schema)
		.map_err(|e| Error::Invalid(format!("invalid entity schema: {e}")))?;
	match e.kind.as_str() {
		"embedding" => serde_json::from_value::<crate::semantic::EmbeddingConfig>(e.config.clone())
			.map_err(|e| Error::Invalid(e.to_string()))?
			.validate_in(local)?,
		"compactor" => serde_json::from_value::<CompactorConfig>(e.config.clone())
			.map_err(|e| Error::Invalid(e.to_string()))?
			.validate_in(local)?,
		"model" => {
			let m: ModelConfig = serde_json::from_value(e.config.clone())
				.map_err(|e| Error::Invalid(e.to_string()))?;
			if m.provider != "openrouter"
				|| m.model_id.trim().is_empty()
				|| m.context_window < 2048
				|| (local && m.max_output_tokens.is_none())
				|| m.max_output_tokens
					.is_some_and(|tokens| tokens == 0 || tokens as usize > m.context_window)
				|| !m.modalities.iter().any(|m| m == "text")
			{
				return Err(Error::Invalid(
					"model requires openrouter, model_id, text modality, context_window >= 2048 and a valid max_output_tokens value"
						.into(),
				));
			}
			m.request_timeout()?;
			validate_endpoint(&m.endpoint)?;
			if let Some(name) = m.credential_env {
				crate::config::validate_secret_reference(&name)?;
				if local {
					secret(&name)?;
				}
			}
		}
		"agent" => {
			let a: AgentConfig = serde_json::from_value(e.config.clone())
				.map_err(|e| Error::Invalid(e.to_string()))?;
			if (a.instructions.trim().is_empty() && a.skills.is_empty())
				|| !(1..=1000).contains(&a.max_steps)
			{
				return Err(Error::Invalid(
					"agent requires skills or additional instructions and max_steps in 1..1000"
						.into(),
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
		"tool" => crate::tool::validate_config_in(&e.config, local)?,
		"skill" => {
			let instructions = e
				.config
				.get("instructions")
				.and_then(Value::as_str)
				.ok_or_else(|| Error::Invalid("skill requires instructions".into()))?;
			if instructions.trim().is_empty() {
				return Err(Error::Invalid("skill requires instructions".into()));
			}
			if instructions.len() > 65_536 || instructions.contains('\0') {
				return Err(Error::Invalid(
					"skill instructions exceed 64 KiB or contain NUL".into(),
				));
			}
			let files = skill_files(e)?;
			if files.len() > 64 {
				return Err(Error::Invalid("skill files exceed 64 files".into()));
			}
			let mut total_bytes = 0;
			let mut paths = std::collections::HashSet::new();
			for file in files {
				total_bytes += file.byte_len()?;
				if total_bytes > 256_000 {
					return Err(Error::Invalid("skill files exceed 256 KB".into()));
				}
				if !valid_skill_file_path(&file.path)
					|| file.content.contains('\0')
					|| !paths.insert(file.path)
				{
					return Err(Error::Invalid(
						"skill file paths must be unique relative paths".into(),
					));
				}
			}
		}
		_ => {}
	}
	Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
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

#[derive(sqlx::FromRow)]
struct StoredPackageRecord {
	id: String,
	version: String,
	manifest: Value,
	digest: String,
	manifest_source: String,
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
		let manifest_source = value.to_string();
		let hash = digest(&value);
		let mut tx = pool.begin().await?;
		let inserted = sqlx::query(
			&sea_orm::sea_query::Query::insert()
				.into_table(sea_orm::sea_query::Alias::new("packages"))
				.columns([
					sea_orm::sea_query::Alias::new("id"),
					sea_orm::sea_query::Alias::new("version"),
					sea_orm::sea_query::Alias::new("manifest"),
					sea_orm::sea_query::Alias::new("digest"),
					sea_orm::sea_query::Alias::new("manifest_source"),
				])
				.values_panic([
					sea_orm::sea_query::Expr::cust("$1"),
					sea_orm::sea_query::Expr::cust("$2"),
					sea_orm::sea_query::Expr::cust("$3"),
					sea_orm::sea_query::Expr::cust("$4"),
					sea_orm::sea_query::Expr::cust("$5"),
				])
				.on_conflict(
					sea_orm::sea_query::OnConflict::new()
						.do_nothing()
						.to_owned(),
				)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(&package.entity.id)
		.bind(&package.entity.version)
		.bind(&value)
		.bind(&hash)
		.bind(&manifest_source)
		.execute(&mut *tx)
		.await?;
		let stored = sqlx::query_as::<_, StoredPackageRecord>(
			&sea_orm::sea_query::Query::select()
				.columns([
					sea_orm::sea_query::Alias::new("id"),
					sea_orm::sea_query::Alias::new("version"),
					sea_orm::sea_query::Alias::new("manifest"),
					sea_orm::sea_query::Alias::new("digest"),
					sea_orm::sea_query::Alias::new("manifest_source"),
				])
				.from(sea_orm::sea_query::Alias::new("packages"))
				.and_where(sea_orm::sea_query::Expr::cust("id = $1 AND version = $2"))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(&package.entity.id)
		.bind(&package.entity.version)
		.fetch_one(&mut *tx)
		.await?;
		if stored.digest != hash || stored.manifest_source != manifest_source {
			return Err(Error::Conflict("package version is immutable".into()));
		}
		if inserted.rows_affected() > 0 {
			package_event(
				&mut tx,
				&self.node_id,
				"package.published",
				json!({"id":stored.id,"version":stored.version}),
			)
			.await?;
		}
		tx.commit().await?;
		Ok(PackageRecord {
			id: stored.id,
			version: stored.version,
			manifest: stored.manifest,
			digest: stored.digest,
		})
	}
	pub async fn install(
		&self,
		pool: &sqlx::PgPool,
		id: &str,
		version: &str,
		expected_digest: &str,
		config: Value,
	) -> Result<Entry> {
		let record = sqlx::query_as::<_, StoredPackageRecord>(
			&sea_orm::sea_query::Query::select()
				.columns([
					sea_orm::sea_query::Alias::new("id"),
					sea_orm::sea_query::Alias::new("version"),
					sea_orm::sea_query::Alias::new("manifest"),
					sea_orm::sea_query::Alias::new("digest"),
					sea_orm::sea_query::Alias::new("manifest_source"),
				])
				.from(sea_orm::sea_query::Alias::new("packages"))
				.and_where(sea_orm::sea_query::Expr::cust("id = $1 AND version = $2"))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(id)
		.bind(version)
		.fetch_optional(pool)
		.await?
		.ok_or_else(|| Error::NotFound("package".into()))?;
		let source_digest = format!(
			"sha256:{:x}",
			Sha256::digest(record.manifest_source.as_bytes())
		);
		let source_manifest: Value = serde_json::from_str(&record.manifest_source)?;
		if record.digest != expected_digest
			|| source_digest != expected_digest
			|| source_manifest != record.manifest
		{
			return Err(Error::Conflict("package digest changed".into()));
		}
		let package: Package = serde_json::from_value(source_manifest)?;
		let mut effective = package.entity.clone();
		overlay_config(&mut effective.config, &config)?;
		self.validate_references(&effective).await?;
		for dep in &package.dependencies {
			self.get(&dep.id, &dep.version).await?;
		}
		let mut tx = pool.begin().await?;
		let original = serde_json::to_value(&package.entity)?;
		sqlx::query(
			&sea_orm::sea_query::Query::insert()
				.into_table(sea_orm::sea_query::Alias::new("registry"))
				.columns([
					sea_orm::sea_query::Alias::new("id"),
					sea_orm::sea_query::Alias::new("version"),
					sea_orm::sea_query::Alias::new("kind"),
					sea_orm::sea_query::Alias::new("metadata"),
				])
				.values_panic([
					sea_orm::sea_query::Expr::cust("$1"),
					sea_orm::sea_query::Expr::cust("$2"),
					sea_orm::sea_query::Expr::cust("$3"),
					sea_orm::sea_query::Expr::cust("$4"),
				])
				.on_conflict(
					sea_orm::sea_query::OnConflict::new()
						.do_nothing()
						.to_owned(),
				)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(id)
		.bind(version)
		.bind(&package.entity.kind)
		.bind(&original)
		.execute(&mut *tx)
		.await?;
		let stored: Value = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("metadata")),
				))
				.from(sea_orm::sea_query::Alias::new("registry"))
				.and_where(sea_orm::sea_query::Expr::cust("id = $1 AND version = $2"))
				.lock(sea_orm::sea_query::LockType::Update)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(id)
		.bind(version)
		.fetch_one(&mut *tx)
		.await?;
		if stored != original {
			return Err(Error::Conflict(
				"package entity conflicts with immutable registry version".into(),
			));
		}
		let changed = sqlx::query(
			&sea_orm::sea_query::Query::insert()
				.into_table(sea_orm::sea_query::Alias::new("installations"))
				.columns([
					sea_orm::sea_query::Alias::new("id"),
					sea_orm::sea_query::Alias::new("version"),
					sea_orm::sea_query::Alias::new("digest"),
					sea_orm::sea_query::Alias::new("config"),
				])
				.values_panic([
					sea_orm::sea_query::Expr::cust("$1"),
					sea_orm::sea_query::Expr::cust("$2"),
					sea_orm::sea_query::Expr::cust("$3"),
					sea_orm::sea_query::Expr::cust("$4"),
				])
				.on_conflict(
					sea_orm::sea_query::OnConflict::columns([
						sea_orm::sea_query::Alias::new("id"),
						sea_orm::sea_query::Alias::new("version"),
					])
					.value(
						sea_orm::sea_query::Alias::new("config"),
						sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col((
							sea_orm::sea_query::Alias::new("excluded"),
							sea_orm::sea_query::Alias::new("config"),
						))),
					)
					.action_and_where(sea_orm::sea_query::Expr::cust(
						"installations.config IS DISTINCT FROM EXCLUDED.config",
					))
					.to_owned(),
				)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(id)
		.bind(version)
		.bind(expected_digest)
		.bind(config)
		.execute(&mut *tx)
		.await?;
		if changed.rows_affected() > 0 {
			package_event(
				&mut tx,
				&self.node_id,
				"package.installed",
				json!({"id":id,"version":version}),
			)
			.await?;
		}
		tx.commit().await?;
		Ok(effective)
	}
}

async fn package_event(
	tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
	node: &str,
	kind: &str,
	data: Value,
) -> Result<()> {
	sqlx::query(
		&sea_orm::sea_query::Query::insert()
			.into_table(sea_orm::sea_query::Alias::new("events"))
			.columns([
				sea_orm::sea_query::Alias::new("id"),
				sea_orm::sea_query::Alias::new("node_id"),
				sea_orm::sea_query::Alias::new("kind"),
				sea_orm::sea_query::Alias::new("data"),
			])
			.values_panic([
				sea_orm::sea_query::Expr::cust("$1"),
				sea_orm::sea_query::Expr::cust("$2"),
				sea_orm::sea_query::Expr::cust("$3"),
				sea_orm::sea_query::Expr::cust("$4"),
			])
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(uuid::Uuid::new_v4())
	.bind(node)
	.bind(kind)
	.bind(data)
	.execute(&mut **tx)
	.await?;
	Ok(())
}

/// Transactional registration for compound admission. References and metadata
/// use the same immutable version contract as the public Registry operation.
/// Keep server-assigned IDs stable across retries, atomically with registration.
pub(crate) async fn assign_id_in(
	tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
	entry: &mut Entry,
	key: Option<uuid::Uuid>,
) -> Result<()> {
	use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
	let request = serde_json::to_value(&*entry)?;
	if entry.id.is_empty() {
		entry.id = uuid::Uuid::now_v7().to_string();
	}
	let Some(key) = key else {
		return Ok(());
	};
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("registry_requests"))
			.columns([
				Alias::new("key"),
				Alias::new("request"),
				Alias::new("entity_id"),
			])
			.values_panic([Expr::cust("$1"), Expr::cust("$2"), Expr::cust("$3")])
			.on_conflict(
				OnConflict::column(Alias::new("key"))
					.do_nothing()
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.bind(key)
	.bind(&request)
	.bind(&entry.id)
	.execute(&mut **tx)
	.await?;
	// A conflicting insert waits for the winning transaction before this read.
	let (stored, id): (Value, String) = sqlx::query_as(
		&Query::select()
			.columns([Alias::new("request"), Alias::new("entity_id")])
			.from(Alias::new("registry_requests"))
			.and_where(Expr::col(Alias::new("key")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(key)
	.fetch_one(&mut **tx)
	.await?;
	if stored != request {
		return Err(Error::Conflict(
			"registration idempotency key reused with different input".into(),
		));
	}
	entry.id = id;
	Ok(())
}

pub(crate) async fn register_in(
	tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
	entry: &Entry,
	node_id: &str,
) -> Result<bool> {
	validate(entry)?;
	if entry.kind == "tool"
		&& let crate::tool::ToolConfig::Agent {
			node_id: target,
			agent,
		} = serde_json::from_value(entry.config.clone())?
		&& target == node_id
	{
		use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
		let kind: Option<String> = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("kind"))
				.from(Alias::new("registry"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.and_where(Expr::col(Alias::new("version")).eq(Expr::cust("$2")))
				.to_string(PostgresQueryBuilder),
		)
		.bind(&agent.id)
		.bind(&agent.version)
		.fetch_optional(&mut **tx)
		.await?;
		if kind.as_deref() != Some("agent") {
			return Err(Error::Invalid(
				"agent tool executor must reference a local agent".into(),
			));
		}
	}
	if entry.kind == "agent" {
		let config: AgentConfig = serde_json::from_value(entry.config.clone())?;
		let mut references = Vec::new();
		for (reference, kind) in std::iter::once((&config.model, "model"))
			.chain(config.tools.iter().map(|r| (r, "tool")))
			.chain(config.skills.iter().map(|r| (r, "skill")))
			.chain(config.cluster.iter().map(|r| (r, "cluster")))
		{
			let actual: Option<Value> = sqlx::query_scalar(
				&sea_orm::sea_query::Query::select()
					.expr(sea_orm::sea_query::SimpleExpr::from(
						sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("metadata")),
					))
					.from(sea_orm::sea_query::Alias::new("registry"))
					.and_where(sea_orm::sea_query::Expr::cust("id = $1 AND version = $2"))
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(&reference.id)
			.bind(&reference.version)
			.fetch_optional(&mut **tx)
			.await?;
			let mut referenced: Entry = serde_json::from_value(
				actual.ok_or_else(|| Error::NotFound(reference.id.clone()))?,
			)?;
			if referenced.kind != kind {
				return Err(Error::Invalid(format!(
					"{} must reference a {kind}",
					reference.id
				)));
			}
			use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
			let overrides: Option<Value> = sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("config"))
					.from(Alias::new("installations"))
					.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
					.and_where(Expr::col(Alias::new("version")).eq(Expr::cust("$2")))
					.to_string(PostgresQueryBuilder),
			)
			.bind(&reference.id)
			.bind(&reference.version)
			.fetch_optional(&mut **tx)
			.await?;
			if let Some(overrides) = overrides {
				overlay_config(&mut referenced.config, &overrides)?;
			}
			references.push(referenced);
		}
		validate_agent_prompt(&config, &references, &Value::Null)?;
	}
	if entry.kind == "cluster" {
		let config: ClusterConfig = serde_json::from_value(entry.config.clone())?;
		let kind: Option<String> = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("kind")),
				))
				.from(sea_orm::sea_query::Alias::new("registry"))
				.and_where(sea_orm::sea_query::Expr::cust("id = $1 AND version = $2"))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
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
	let inserted = sqlx::query(
		&sea_orm::sea_query::Query::insert()
			.into_table(sea_orm::sea_query::Alias::new("registry"))
			.columns([
				sea_orm::sea_query::Alias::new("id"),
				sea_orm::sea_query::Alias::new("version"),
				sea_orm::sea_query::Alias::new("kind"),
				sea_orm::sea_query::Alias::new("metadata"),
			])
			.values_panic([
				sea_orm::sea_query::Expr::cust("$1"),
				sea_orm::sea_query::Expr::cust("$2"),
				sea_orm::sea_query::Expr::cust("$3"),
				sea_orm::sea_query::Expr::cust("$4"),
			])
			.on_conflict(
				sea_orm::sea_query::OnConflict::new()
					.do_nothing()
					.to_owned(),
			)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(&entry.id)
	.bind(&entry.version)
	.bind(&entry.kind)
	.bind(&value)
	.execute(&mut **tx)
	.await?
	.rows_affected()
		!= 0;
	let stored: Value = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("metadata")),
			))
			.from(sea_orm::sea_query::Alias::new("registry"))
			.and_where(sea_orm::sea_query::Expr::cust("id = $1 AND version = $2"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(&entry.id)
	.bind(&entry.version)
	.fetch_one(&mut **tx)
	.await?;
	if stored != value {
		return Err(Error::Conflict(
			"published versions are immutable; choose a new version".into(),
		));
	}
	Ok(inserted)
}

fn overlay_config(target: &mut Value, overrides: &Value) -> Result<()> {
	let object = overrides
		.as_object()
		.ok_or_else(|| Error::Invalid("installation config must be an object".into()))?;
	if object.contains_key("knowledge_digest") {
		return Err(Error::Invalid(
			"installation config cannot override immutable knowledge_digest".into(),
		));
	}
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
	#[rstest::rstest]
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
	#[rstest::rstest]
	fn bundled_skill_files_have_bounded_safe_paths_and_are_listed_on_demand() {
		let mut e = entry();
		e.config = json!({"instructions":"Read the guide when relevant","files":[{"path":"references/guide.md","content":"Evidence"}]});
		validate(&e).unwrap();
		assert!(
			skill_instructions(&e)
				.unwrap()
				.contains("references/guide.md")
		);
		assert!(!skill_instructions(&e).unwrap().contains("Evidence"));
		e.config["files"] =
			json!([{"path":"assets/logo.png","content":"AAEC","encoding":"base64"}]);
		validate(&e).unwrap();
		assert!(
			skill_instructions(&e)
				.unwrap()
				.contains("binary, base64 encoded")
		);
		e.config["files"][0]["content"] = json!("not base64");
		assert!(validate(&e).is_err());
		e.config["files"][0]["content"] = json!("AAEC");
		e.config["files"][0]["encoding"] = json!("unknown");
		assert!(validate(&e).is_err());
		e.config["files"] = json!([{"path":"references/guide.md","content":"Evidence"}]);
		for path in [
			"../secret",
			"/absolute",
			"references/../secret",
			"references\\secret",
			"references/.hidden",
		] {
			e.config["files"][0]["path"] = json!(path);
			assert!(validate(&e).is_err(), "{path}");
		}
	}
	#[rstest::rstest]
	fn skills_only_agents_do_not_need_custom_prompts() {
		let mut e = entry();
		e.kind = "agent".into();
		e.config = json!({"model":{"id":"model","version":"1.0.0"},"skills":[{"id":"research","version":"1.0.0"}]});
		assert!(validate(&e).is_ok());
		e.config["skills"] = json!([]);
		assert!(validate(&e).is_err());
		e.config["instructions"] = json!("Legacy instructions");
		assert!(validate(&e).is_ok());
	}
	#[rstest::rstest]
	fn rejects_invalid_metadata() {
		let mut e = entry();
		e.version = "latest".into();
		assert!(validate(&e).is_err());
		e.version = "1.0.0".into();
		e.id = "../../escape".into();
		assert!(validate(&e).is_err());
	}
	#[rstest::rstest]
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
	#[rstest::rstest]
	fn installation_overrides_cannot_clear_a_personal_agent_knowledge_digest() {
		let mut config = json!({"knowledge_digest":"immutable-digest"});
		assert!(overlay_config(&mut config, &json!({"knowledge_digest":null})).is_err());
		assert_eq!(config["knowledge_digest"], "immutable-digest");
		assert!(overlay_config(&mut config, &json!({"display_name":"Local name"})).is_ok());
		assert_eq!(config["display_name"], "Local name");
	}
}

#[derive(Serialize, Deserialize)]
pub struct AgentPage {
	pub entries: Vec<Entry>,
	pub next_offset: Option<u64>,
}

pub(crate) fn validate_agent_prompt(
	config: &AgentConfig,
	references: &[Entry],
	private_context: &Value,
) -> Result<()> {
	agent_prompt_headroom(config, references, private_context).map(|_| ())
}

pub(crate) fn agent_prompt_headroom(
	config: &AgentConfig,
	references: &[Entry],
	private_context: &Value,
) -> Result<usize> {
	let get = |reference: &EntityRef| {
		references
			.iter()
			.find(|e| e.id == reference.id && e.version == reference.version)
			.ok_or_else(|| Error::NotFound(reference.id.clone()))
	};
	let model: ModelConfig = serde_json::from_value(get(&config.model)?.config.clone())?;
	let mut instructions = crate::context::agent_instructions("");
	for skill in &config.skills {
		instructions.push('\n');
		instructions.push_str(&format!("Skill {}@{}:\n", skill.id, skill.version));
		instructions.push_str(&skill_instructions(get(skill)?)?);
	}
	instructions.push_str("\nAdditional user instructions:\n");
	instructions.push_str(&config.instructions);
	let mut specifications = crate::tool::builtins()
		.values()
		.map(|t| t.specification())
		.collect::<Vec<_>>();
	for (index, tool) in config.tools.iter().enumerate() {
		specifications.push(crate::tool::plugin_specification(
			get(tool)?,
			&format!("plugin_{index}"),
		));
	}
	crate::context::request_context_budget(
		model.context_window,
		model.output_token_limit(),
		&instructions,
		&specifications,
		private_context,
	)
}
