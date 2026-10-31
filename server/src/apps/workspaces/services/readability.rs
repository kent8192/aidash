//! Workspace-level, advisory attachment readability; no file bytes or authority grants.
use super::{ChannelAttachment, access::Lease};
use crate::Result;
use aidash_domain::{
	model::ModelConfig,
	registry::{
		Entry,
		bindings::{BindingKind, BindingSnapshot, ForeignAgentSnapshot},
	},
};
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, JoinType, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	TableRef,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

struct Reader {
	select: bool,
	copy: bool,
	model: Option<ModelConfig>,
}
impl Reader {
	fn reads(&self, media_type: &str, duration_ms: Option<i64>) -> bool {
		if !media_type.starts_with("image/") && !media_type.starts_with("audio/") {
			return self.copy;
		}
		if !self.select {
			return false;
		}
		let Some(model) = &self.model else {
			return false;
		};
		if model.require_media_types([media_type]).is_ok() {
			return true;
		}
		media_type.starts_with("audio/")
			&& !matches!(media_type, "audio/wav" | "audio/x-wav")
			&& duration_ms.is_some_and(|ms| {
				ms >= 0 && (ms as u64).saturating_mul(32).saturating_add(44) <= 8 * 1024 * 1024
			}) && model.require_media_types(["audio/wav"]).is_ok()
	}
	fn from_snapshot(snapshot: &BindingSnapshot) -> Self {
		let select = snapshot.supports_thread_attachment_selection();
		let copy = !snapshot.remote
			&& snapshot.bindings.iter().any(|b| {
				b.excluded_reason.is_none()
					&& b.identity.registry_deployment == snapshot.agent.registry_deployment
					&& b.identity.id == "aidash.attachment_copy"
					&& b.identity.version == "1.0.0"
					&& b.definition.kind == "tool"
			});
		let model_ref = snapshot
			.definitions
			.iter()
			.find(|d| d.identity == snapshot.agent)
			.map(|d| &d.definition.config["model"]);
		let model = model_ref
			.and_then(|r| {
				snapshot.definitions.iter().find(|d| {
					d.definition.kind == "model"
						&& Some(d.identity.id.as_str()) == r["id"].as_str()
						&& Some(d.identity.version.as_str()) == r["version"].as_str()
				})
			})
			.and_then(|d| serde_json::from_value(d.definition.config.clone()).ok());
		Self {
			select,
			copy,
			model,
		}
	}
	fn from_entry(
		agent: &Entry,
		node: &str,
		definitions: &HashMap<(String, String), Entry>,
	) -> Self {
		let select = agent
			.supports_thread_attachment_selection(node)
			.unwrap_or(false);
		let copy = agent.normalized_bindings(node).is_ok_and(|items| {
			items.iter().any(|b| {
				b.binding.kind == BindingKind::Tool
					&& b.binding.target.registry_deployment == node
					&& b.binding.target.id == "aidash.attachment_copy"
					&& b.binding.target.version == "1.0.0"
			})
		});
		let model = agent
			.config
			.get("model")
			.and_then(|r| {
				Some((
					r.get("id")?.as_str()?.to_owned(),
					r.get("version")?.as_str()?.to_owned(),
				))
			})
			.and_then(|key| definitions.get(&key))
			.and_then(|e| serde_json::from_value(e.config.clone()).ok());
		Self {
			select,
			copy,
			model,
		}
	}
}

/// Resolve all Workspace assignments and definitions in bounded query batches.
pub(crate) async fn annotate(
	lease: &mut Lease,
	workspace: Uuid,
	node: &str,
	attachments: &mut [&mut ChannelAttachment],
) -> Result<()> {
	if attachments.is_empty() {
		return Ok(());
	}
	let tenant = lease.access_mut().identity.tenant.clone();
	let rows: Vec<(String, String, String, Option<Value>)> = crate::database::native::query_as(
		&Query::select()
			.columns([
				(Alias::new("d"), Alias::new("node_id")),
				(Alias::new("d"), Alias::new("agent_id")),
				(Alias::new("d"), Alias::new("agent_version")),
				(Alias::new("d"), Alias::new("binding_snapshot")),
			])
			.from_as(Alias::new("delegations"), Alias::new("d"))
			.join(
				JoinType::InnerJoin,
				TableRef::table_alias(Alias::new("tasks"), Alias::new("t")),
				Expr::col((Alias::new("t"), Alias::new("id")))
					.equals((Alias::new("d"), Alias::new("task_id"))),
			)
			.and_where(
				Expr::col((Alias::new("t"), Alias::new("workspace_id"))).eq(Expr::value(workspace)),
			)
			.and_where(
				Expr::col((Alias::new("d"), Alias::new("tenant"))).eq(Expr::value(tenant.clone())),
			)
			.to_string(PostgresQueryBuilder),
	)
	.columns(&["node_id", "agent_id", "agent_version", "binding_snapshot"])
	.fetch_all(&mut **lease.tx())
	.await?;
	let mut local = HashSet::new();
	let mut readers = Vec::new();
	for (assigned_node, id, version, snapshot) in rows {
		if aidash_domain::nodes::same_deployment(&assigned_node, node) {
			local.insert((id, version));
		} else if let Some(snapshot) =
			snapshot.and_then(|value| serde_json::from_value::<ForeignAgentSnapshot>(value).ok())
		{
			readers.push(Reader::from_snapshot(&snapshot.snapshot()));
		}
	}
	let targets: Vec<String> = crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("target"))
			.from(Alias::new("conversations"))
			.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::value(workspace)))
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant.clone())))
			.and_where(Expr::col(Alias::new("target_kind")).eq("agent"))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_all(&mut **lease.tx())
	.await?;
	for target in targets {
		if let Some((id, version)) = target.rsplit_once('@') {
			local.insert((id.to_owned(), version.to_owned()));
		}
	}
	let ids: Vec<_> = local.iter().map(|(id, _)| id.clone()).collect();
	let mut definitions = HashMap::new();
	if !ids.is_empty() {
		let values: Vec<Value> = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("metadata"))
				.from(Alias::new("registry"))
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant.clone())))
				.and_where(Expr::col(Alias::new("id")).is_in(ids.into_iter().map(Expr::value)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_all(&mut **lease.tx())
		.await?;
		for value in values {
			let entry: Entry = serde_json::from_value(value)?;
			definitions.insert((entry.id.clone(), entry.version.clone()), entry);
		}
		let models: Vec<_> = local
			.iter()
			.filter_map(|key| definitions.get(key))
			.filter_map(|entry| entry.config["model"]["id"].as_str().map(str::to_owned))
			.collect();
		if !models.is_empty() {
			let values: Vec<Value> = crate::database::native::query_scalar(
				&Query::select()
					.column(Alias::new("metadata"))
					.from(Alias::new("registry"))
					.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
					.and_where(
						Expr::col(Alias::new("id")).is_in(models.into_iter().map(Expr::value)),
					)
					.to_string(PostgresQueryBuilder),
			)
			.scalar_all(&mut **lease.tx())
			.await?;
			for value in values {
				let entry: Entry = serde_json::from_value(value)?;
				definitions.insert((entry.id.clone(), entry.version.clone()), entry);
			}
		}
		for key in local {
			if let Some(agent) = definitions.get(&key) {
				readers.push(Reader::from_entry(agent, node, &definitions));
			}
		}
	}
	let ids: Vec<_> = attachments.iter().map(|a| a.id).collect();
	let durations: Vec<(Uuid, Option<i64>)> = crate::database::native::query_as(
		&Query::select()
			.columns([Alias::new("id"), Alias::new("duration_ms")])
			.from(Alias::new("channel_attachments"))
			.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::value(workspace)))
			.and_where(Expr::col(Alias::new("id")).is_in(ids.into_iter().map(Expr::value)))
			.to_string(PostgresQueryBuilder),
	)
	.columns(&["id", "duration_ms"])
	.fetch_all(&mut **lease.tx())
	.await?;
	let durations: HashMap<_, _> = durations.into_iter().collect();
	for attachment in attachments {
		attachment.no_assigned_agent_can_read = !readers.iter().any(|reader| {
			reader.reads(
				&attachment.media_type,
				durations.get(&attachment.id).copied().flatten(),
			)
		});
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use chrono::{Duration, Utc};
	use serde_json::json;

	fn fixture() -> (Entry, HashMap<(String, String), Entry>) {
		let mut agent: Entry = serde_json::from_value(json!({"id":"agent", "version":"1.0.0", "kind":"agent", "name":{"en":"Agent"}, "description":{"en":"Agent"}, "schema":{}, "config":{"schema_version":1,"instructions":"Work", "model":{"id":"model", "version":"1.0.0"}}})).unwrap();
		agent.normalize_agent("aidash://local").unwrap();
		let model: Entry = serde_json::from_value(json!({"id":"model", "version":"1.0.0", "kind":"model", "name":{"en":"Model"}, "description":{"en":"Model"}, "schema":{}, "config":{"provider":"openrouter", "model_id":"example", "endpoint":"https://example.com", "context_window":128000, "modalities":["text","image","audio"], "media_routes":[{"tag":"route", "formats":["image/png", "wav"], "source":"fixture", "verified_at":Utc::now()-Duration::hours(1), "expires_at":Utc::now()+Duration::hours(1)}], "cost":{}}})).unwrap();
		(
			agent,
			[((model.id.clone(), model.version.clone()), model)].into(),
		)
	}
	#[test]
	fn pinned_tools_modalities_and_routes_control_readability() {
		let (mut agent, mut definitions) = fixture();
		let reader = Reader::from_entry(&agent, "aidash://local", &definitions);
		assert!(reader.reads("image/png", None));
		assert!(reader.reads("application/pdf", None));
		assert!(reader.reads("audio/ogg", Some(1000)));
		assert!(!reader.reads("audio/ogg", Some(300_000)));
		assert!(!reader.reads("audio/ogg", None));
		let mut expired = definitions.clone();
		expired
			.get_mut(&("model".into(), "1.0.0".into()))
			.unwrap()
			.config["media_routes"][0]["expires_at"] = json!(Utc::now() - Duration::minutes(1));
		assert!(!Reader::from_entry(&agent, "aidash://local", &expired).reads("image/png", None));
		definitions
			.get_mut(&("model".into(), "1.0.0".into()))
			.unwrap()
			.config["modalities"] = json!(["text"]);
		assert!(
			!Reader::from_entry(&agent, "aidash://local", &definitions).reads("image/png", None)
		);
		definitions
			.get_mut(&("model".into(), "1.0.0".into()))
			.unwrap()
			.config["modalities"] = json!(["text", "image"]);
		let saved = agent.binding_normalization.as_mut().unwrap();
		saved
			.bindings
			.retain(|b| b.binding.target.id != "aidash.attachment_copy");
		for b in &mut saved.bindings {
			if matches!(
				b.binding.target.id.as_str(),
				"aidash.workspace_read" | "aidash.workspace_observe" | "aidash.workspace_message"
			) {
				b.binding.target.version = "1.0.0".into();
			}
		}
		let legacy = Reader::from_entry(&agent, "aidash://local", &definitions);
		assert!(!legacy.reads("image/png", None));
		assert!(!legacy.reads("application/pdf", None));
	}
	#[test]
	fn remote_agent_does_not_make_non_media_files_readable() {
		use aidash_domain::registry::bindings::{
			BindingOrigin, Narrowing, QualifiedRef, ResolvedBinding,
		};
		let (mut definition, _) = fixture();
		definition.kind = "tool".into();
		let node = "aidash://remote";
		let mut snapshot = BindingSnapshot {
			schema_version: 1,
			agent: QualifiedRef {
				registry_deployment: node.into(),
				id: "agent".into(),
				version: "1.0.0".into(),
			},
			remote: false,
			bindings: vec![ResolvedBinding {
				identity: QualifiedRef::builtin(node, "attachment_copy"),
				definition,
				digest: String::new(),
				origin: BindingOrigin::Default,
				alias: None,
				narrow: Narrowing::default(),
				installation: None,
				provider_contract_digest: None,
				provider_implementation: None,
				excluded_reason: None,
			}],
			definitions: vec![],
			foreign_agents: vec![],
			agent_plugins: vec![],
		};
		assert!(Reader::from_snapshot(&snapshot).reads("application/pdf", None));
		snapshot.remote = true;
		assert!(!Reader::from_snapshot(&snapshot).reads("application/pdf", None));
	}
}
