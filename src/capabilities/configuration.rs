//! Subject-scoped immutable configuration revisions. Editing a capability flag
//! does not create a policy grant or enable the new version in a tenant catalog.
use super::{CoreCapabilities, references, sessions, skills};
use crate::{
	Error, Result,
	authorization::{access::Access, catalog},
	registry::{AgentConfig, EntityRef},
	store::Store,
};
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Configure {
	pub idempotency_key: Uuid,
	pub source_version: String,
	pub new_version: String,
	pub core_capabilities: CoreCapabilities,
	pub skill_attachments: Vec<skills::SkillAttachment>,
	pub skill_roots: Vec<String>,
	pub reference_attachments: Vec<references::Attachment>,
}
pub(crate) async fn configure(
	store: &Store,
	access: &mut Access,
	id: String,
	input: Configure,
) -> Result<Value> {
	let digest = crate::registry::digest(&json!(["capability_configuration", id, input]));
	let mut entry = catalog::entry(
		access,
		&EntityRef {
			id: id.clone(),
			version: input.source_version.clone(),
		},
		"agent.configure",
	)
	.await?;
	if entry.kind != "agent" {
		return Err(Error::Forbidden);
	}
	if let Some(cached) = sessions::cached(access, input.idempotency_key, &digest).await? {
		return Ok(cached);
	}
	if input.new_version == input.source_version {
		return Err(Error::Conflict("AGENT_VERSION_IMMUTABLE".into()));
	}
	let mut config: AgentConfig = serde_json::from_value(entry.config.clone())?;
	config.core_capabilities = input.core_capabilities;
	config.skill_attachments = input.skill_attachments;
	config.skill_roots = input.skill_roots;
	config.reference_attachments = input.reference_attachments;
	skills::validate_config(&config)?;
	references::validate_config(&config)?;
	if config.reference_attachments.len() > store.capabilities.0.limits.reference_files {
		return Err(Error::Invalid("REFERENCE_SET_LIMIT".into()));
	}
	let mut extracted = 0;
	for binding in &config.reference_attachments {
		let record = references::get(access, binding.reference_id, "reference.read").await?;
		if record.state != "ready" || record.data["original"]["digest"] != binding.digest {
			return Err(Error::Conflict("REFERENCE_NOT_READY_OR_CHANGED".into()));
		}
		if record.data["original"]["size"].as_u64().unwrap_or(u64::MAX)
			> store.capabilities.0.limits.reference_bytes
		{
			return Err(Error::Invalid("REFERENCE_SET_LIMIT".into()));
		}
		extracted += record.data["extraction"]["size"].as_u64().unwrap_or(0);
	}
	if extracted > store.capabilities.0.limits.reference_text_bytes as u64 {
		return Err(Error::Invalid("REFERENCE_SET_LIMIT".into()));
	}
	entry.version = input.new_version;
	entry.config = serde_json::to_value(config)?;
	let inserted = crate::registry::register_in(&mut access.tx, &entry, &store.node_id).await?;
	if inserted {
		// Preserve historical text-only attachments exactly, without fabricating originals.
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("agent_knowledge"))
				.columns(["agent_id", "agent_version", "documents"].map(Alias::new))
				.select_from(
					Query::select()
						.expr(Expr::cust("$1"))
						.expr(Expr::cust("$2"))
						.column(Alias::new("documents"))
						.from(Alias::new("agent_knowledge"))
						.and_where(Expr::col(Alias::new("agent_id")).eq(Expr::cust("$1")))
						.and_where(Expr::col(Alias::new("agent_version")).eq(Expr::cust("$3")))
						.to_owned(),
				)
				.map_err(|_| Error::Invalid("configuration copy".into()))?
				.to_string(PostgresQueryBuilder),
		)
		.bind(&id)
		.bind(&entry.version)
		.bind(&input.source_version)
		.execute(&mut **access.tx)
		.await?;
		store.event(&mut access.tx,None,"capability.agent_configured",json!({"agent_id":id,"source_version":input.source_version,"version":entry.version,"actor":access.identity.subject})).await?;
	}
	let result = json!({"entry":entry,"catalog_approval_required":true});
	sessions::cache(access, input.idempotency_key, &digest, &result).await?;
	Ok(result)
}
