//! Subject-scoped immutable configuration revisions. Editing a capability flag
//! does not create a policy grant or enable the new version in a tenant catalog.
use super::{references, sessions, skills};
use crate::{
	Error, Result,
	authorization::{access::Access, catalog},
	registry::{AgentConfig, EntityRef},
	store::Store,
};
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde_json::{Value, json};

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
	if entry.installation.is_some() {
		return Err(Error::Invalid(
			"use installation.configure for installed definitions".into(),
		));
	}
	entry.version = input.new_version;
	entry.config = serde_json::to_value(config)?;
	let inserted = crate::registry::register_in(&mut access.tx, &entry, &store.node_id).await?;
	if inserted {
		crate::marketplace::propagate_provenance(
			&mut access.tx,
			&EntityRef {
				id: id.clone(),
				version: input.source_version.clone(),
			},
			&entry,
			&access.identity.tenant,
		)
		.await?;
		// Preserve historical text-only attachments exactly, without fabricating originals.
		{
			let query_bind_1 = &id;
			let query_bind_2 = &entry.version;
			let query_bind_3 = &input.source_version;
			sqlx::query(
				&Query::insert()
					.into_table(Alias::new("agent_knowledge"))
					.columns(["agent_id", "agent_version", "documents"].map(Alias::new))
					.from_subquery(
						Query::select()
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							))
							.column(Alias::new("documents"))
							.from(Alias::new("agent_knowledge"))
							.and_where(
								reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
									"agent_id",
								)))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								)),
							)
							.and_where(
								reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
									"agent_version",
								)))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_3.to_owned()).into()],
								)),
							)
							.to_owned(),
					)
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **access.tx)
			.await?
		};
		store.event(&mut access.tx,None,"capability.agent_configured",json!({"agent_id":id,"source_version":input.source_version,"version":entry.version,"actor":access.identity.subject})).await?;
	}
	let result = json!({"entry":entry,"catalog_approval_required":true});
	sessions::cache(access, input.idempotency_key, &digest, &result).await?;
	Ok(result)
}

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};

pub use crate::apps::execution::capabilities::serializers::configuration::Configure;

use reinhardt::query::SimpleExpr;
