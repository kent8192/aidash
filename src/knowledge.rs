//! Private reference text, stored separately from discoverable registry metadata.
use crate::{
	Error, Result,
	federation::Federation,
	registry::{AgentConfig, Entry},
};
use axum::{Json, extract::State, http::HeaderMap};
use sea_orm::{
	ConnectionTrait, DatabaseConnection, DbBackend,
	sea_query::{Alias, Expr, OnConflict, PostgresQueryBuilder, Query},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReferenceDocument {
	pub name: String,
	pub media_type: String,
	/// Text extracted locally in the browser; source binaries are not retained.
	pub text: String,
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PersonalAgent {
	pub entry: Entry,
	pub documents: Vec<ReferenceDocument>,
}
fn validate(documents: &[ReferenceDocument]) -> Result<()> {
	if documents.is_empty()
		|| documents.len() > 8
		|| documents.iter().map(|d| d.text.len()).sum::<usize>() > 65536
	{
		return Err(Error::Invalid(
			"attach 1..8 reference documents with at most 64 KiB of extracted text in total".into(),
		));
	}
	for d in documents {
		if d.name.contains('\0') || d.text.contains('\0') {
			return Err(Error::Invalid("reference document names and text must not contain NUL characters; decode text as UTF-8 before upload".into()));
		}
		if d.name.trim().is_empty()
			|| d.name.len() > 255
			|| d.text.trim().is_empty()
			|| !matches!(
				d.media_type.as_str(),
				"application/pdf"
					| "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
					| "text/plain"
			) {
			return Err(Error::Invalid(
				"invalid or empty reference document; scanned PDFs require OCR before upload"
					.into(),
			));
		}
	}
	Ok(())
}
fn digest(value: &Value) -> String {
	format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}

#[utoipa::path(post, path = "/agents/personal", operation_id = "personal_agent_create", request_body = PersonalAgent, params(("Idempotency-Key" = Uuid, Header)), responses((status=200,body=Entry)), security(("bearer_auth"=[])))]
pub(crate) async fn create(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(mut input): Json<PersonalAgent>,
) -> Result<Json<Entry>> {
	if input.entry.kind != "agent" {
		return Err(Error::Invalid(
			"personal registration requires an agent".into(),
		));
	}
	let _: AgentConfig = serde_json::from_value(input.entry.config.clone())
		.map_err(|e| Error::Invalid(e.to_string()))?;
	validate(&input.documents)?;
	let key = headers
		.get("idempotency-key")
		.and_then(|v| v.to_str().ok())
		.and_then(|v| Uuid::parse_str(v).ok())
		.ok_or_else(|| Error::Invalid("Idempotency-Key must be a UUID".into()))?;
	let documents = serde_json::to_value(&input.documents)?;
	input.entry.config["knowledge_digest"] = json!(digest(&documents));
	let private_context = json!({"reference_documents":documents.clone()});
	let config: AgentConfig = serde_json::from_value(input.entry.config.clone())?;
	let mut references = vec![];
	for reference in std::iter::once(&config.model)
		.chain(config.skills.iter())
		.chain(config.tools.iter())
	{
		references.push(f.registry.get(&reference.id, &reference.version).await?);
	}
	crate::registry::validate_agent_prompt(&config, &references, &private_context)?;
	let mut tx = f.store.pool.begin().await?;
	crate::registry::assign_id_in(&mut tx, &mut input.entry, Some(key)).await?;
	let inserted = crate::registry::register_in(&mut tx, &input.entry, &f.config.node_id).await?;
	let query = Query::insert()
		.into_table(Alias::new("agent_knowledge"))
		.columns([
			Alias::new("agent_id"),
			Alias::new("agent_version"),
			Alias::new("documents"),
		])
		.values_panic([Expr::cust("$1"), Expr::cust("$2"), Expr::cust("$3")])
		.on_conflict(
			OnConflict::columns([Alias::new("agent_id"), Alias::new("agent_version")])
				.do_nothing()
				.to_owned(),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&query)
		.bind(&input.entry.id)
		.bind(&input.entry.version)
		.bind(documents)
		.execute(&mut *tx)
		.await?;
	if inserted {
		f.store
			.event(
				&mut tx,
				None,
				"registry.registered",
				json!({"id":input.entry.id,"version":input.entry.version,"kind":"agent"}),
			)
			.await?;
	}
	tx.commit().await?;
	Ok(Json(input.entry))
}

pub async fn load(db: &DatabaseConnection, entry: &Entry) -> Result<Value> {
	let config: AgentConfig = serde_json::from_value(entry.config.clone())?;
	let Some(expected) = config.knowledge_digest else {
		return Ok(json!([]));
	};
	let query = Query::select()
		.column(Alias::new("documents"))
		.from(Alias::new("agent_knowledge"))
		.and_where(Expr::col(Alias::new("agent_id")).eq(&entry.id))
		.and_where(Expr::col(Alias::new("agent_version")).eq(&entry.version))
		.to_owned();
	let row = db.query_one(DbBackend::Postgres.build(&query)).await?.ok_or_else(|| Error::Invalid("private documents are unavailable on this node; run the original agent on its owning node".into()))?;
	let documents: Value = row.try_get("", "documents")?;
	if digest(&documents) != expected {
		return Err(Error::Conflict("private document digest mismatch".into()));
	}
	Ok(documents)
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn documents_are_bounded_and_nonempty() {
		let mut docs = vec![ReferenceDocument {
			name: "personal.pdf".into(),
			media_type: "application/pdf".into(),
			text: "Reference data".into(),
		}];
		assert!(validate(&docs).is_ok());
		docs[0].text.clear();
		assert!(validate(&docs).is_err());
		docs[0].text = "x".repeat(65537);
		assert!(validate(&docs).is_err());
		assert!(validate(&[]).is_err());
	}
}
