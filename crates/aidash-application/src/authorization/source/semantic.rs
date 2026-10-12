//! Required Home retrieval binds current disclosure authority and exact provider/index definitions.
//! A Summary Stage pin binds the same way, with or without Home retrieval.
use crate::{Error, Result, ports::authorization::source::semantic::SemanticBindingScope};
use aidash_domain::{
	Task,
	federation::execution::Inspection,
	registry::{AgentConfig, rules::digest},
	semantic::{
		EmbeddingConfig, Failure,
		remote::{Binding, Provider, Request, VERSION},
	},
};
use serde_json::{Value, json, to_value};
pub fn requirements(
	inspection: &Inspection,
) -> Result<aidash_domain::federation::execution::AgentMemoryRequirements> {
	let agent = AgentConfig::from_snapshot(&inspection.binding_snapshot)?;
	Ok(
		aidash_domain::federation::execution::AgentMemoryRequirements {
			native_required: agent.memory.is_some()
				&& agent.allow_cross_conversation_memory != Some(false),
			memory_available: inspection.semantic_memory == VERSION
				&& (agent.semantic_memory || agent.workspace_context),
		},
	)
}

pub async fn binding<S: SemanticBindingScope + ?Sized>(
	access: &mut S,
	task: &Task,
	node: &str,
	inspection: &Inspection,
	request: &Request,
) -> Result<Binding> {
	let agent = AgentConfig::from_snapshot(&inspection.binding_snapshot)?;
	let native_enabled = requirements(inspection)?.native_required;
	let (embedding, compactor, native, summarizer) = match request {
		Request::Disabled { summarizer } => {
			if native_enabled {
				return Err(Error::RemoteSemantic(Failure::Configuration));
			}
			let Some(reference) = summarizer else {
				return Ok(Binding::Disabled { summarizer: None });
			};
			// A Summary Stage needs no Home index, embedding or semantic
			// search: only the remote execution and the pinned summarizer's
			// disclosure are authorized.
			let summarizer = summarizer_pin(&agent, inspection, node, reference)?;
			super::authorize(access, task, node, inspection).await?;
			let mut workspace = access.source_workspace(task.workspace_id).await?;
			workspace.attributes["remote_node"] = json!(node);
			workspace.attributes["inference_model"] = json!(agent.model);
			workspace.attributes["compactor"] = Value::Null;
			workspace.attributes["summarizer"] = json!(summarizer.entry);
			access
				.source_require(&workspace, "semantic.disclose")
				.await?;
			return Ok(Binding::Disabled {
				summarizer: Some(Box::new(summarizer)),
			});
		}
		Request::RequiredHome {
			embedding,
			compactor,
			native,
			summarizer,
		} => (embedding, compactor, native, summarizer),
	};
	if native_enabled != native.is_some() {
		return Err(Error::RemoteSemantic(Failure::Configuration));
	}
	if inspection.semantic_memory != VERSION || inspection.compactor.as_ref() != compactor.as_ref()
	{
		return Err(Error::RemoteSemantic(Failure::Configuration));
	}
	if !agent.semantic_memory && !agent.workspace_context {
		return Err(Error::RemoteSemantic(Failure::Configuration));
	}
	let summarizer = summarizer
		.as_ref()
		.map(|reference| summarizer_pin(&agent, inspection, node, reference))
		.transpose()?;
	super::authorize(access, task, node, inspection).await?;
	let mut workspace = access.source_workspace(task.workspace_id).await?;
	workspace.attributes["remote_node"] = json!(node);
	workspace.attributes["inference_model"] = json!(agent.model);
	workspace.attributes["compactor"] = json!(compactor);
	workspace.attributes["summarizer"] = json!(summarizer.as_ref().map(|p| &p.entry));
	access.source_require(&workspace, "semantic.search").await?;
	// Disclosure is independently selectable in policy; local read/search is
	// not permission to send text to the execution node and its providers.
	access
		.source_require(&workspace, "semantic.disclose")
		.await?;
	let (index, spec) = access
		.semantic_index(task.workspace_id)
		.await
		.map_err(|error| {
			if matches!(error, Error::NotFound(_)) {
				Error::RemoteSemantic(Failure::Configuration)
			} else {
				error
			}
		})?;
	if index.tenant != access.binding_tenant()
		|| !spec.enabled
		|| (!native_enabled && !spec.auto_context)
		// RequiredHome search has no local Run admission pin or approved BYOK
		// maintenance authority. Reject before binding/disclosure rather than
		// issuing a remote grant that the environment transport cannot honor.
		|| spec.embedding.provider_credential.is_some()
	{
		return Err(Error::RemoteSemantic(Failure::Configuration));
	}
	let entry = access
		.embedding_entry(embedding, "embedding.invoke")
		.await?;
	access
		.source_require(
			&access.source_resource(
				&entry.kind,
				&entry.id,
				crate::authorization::catalog::attributes(&entry),
			),
			"registry.read",
		)
		.await?;
	if entry.kind != "embedding"
		|| serde_json::from_value::<EmbeddingConfig>(entry.config.clone())? != spec.embedding
	{
		return Err(Error::RemoteSemantic(Failure::Configuration));
	}
	let compactor = if let Some(reference) = compactor {
		let definition = inspection
			.definitions
			.iter()
			.find(|d| d.kind == "compactor" && &d.entry == reference)
			.ok_or(Error::RemoteSemantic(Failure::Configuration))?;
		Some(Provider {
			node_id: node.to_owned(),
			entry: reference.clone(),
			digest: definition.digest.clone(),
			configuration_digest: digest(&definition.metadata.config),
		})
	} else {
		None
	};
	Ok(Binding::RequiredHome {
		native: if let Some(native) = native {
			let generation = inspection
				.generation
				.as_ref()
				.map(|value| -> Result<_> {
					let intent: aidash_domain::generation::intent::Intent =
						serde_json::from_value(value.clone())?;
					Ok(aidash_domain::semantic::remote::NativeOrigin {
						node_id: node.into(),
						intent_id: intent.id,
					})
				})
				.transpose()?;
			Some(Box::new(
				access
					.native_binding(task.workspace_id, native, generation.as_ref())
					.await?,
			))
		} else {
			None
		},
		home_lineage: access.binding_lineage().await?,
		execution_lineage: inspection.lineage.clone(),
		version: VERSION,
		// Native dependencies are canonical Unit revisions. A disposable index
		// rebuild keeps the approved spec and must not withdraw consumed units.
		index_revision: if native_enabled { 0 } else { index.revision },
		index_digest: digest(&index.spec),
		embedding: Box::new(Provider {
			node_id: access.home_node_id().to_owned(),
			entry: embedding.clone(),
			digest: digest(&to_value(&entry)?),
			configuration_digest: digest(&entry.config),
		}),
		compactor: compactor.map(Box::new),
		summarizer: summarizer.map(Box::new),
	})
}

/// Only the exact summarizer pinned by the Agent's Context Policy may be
/// disclosed; its definition is part of the admitted Binding closure.
fn summarizer_pin(
	agent: &AgentConfig,
	inspection: &Inspection,
	node: &str,
	reference: &aidash_domain::registry::EntityRef,
) -> Result<Provider> {
	let pinned = match agent.context_policy.as_ref() {
		Some(aidash_domain::context::policy::ContextPolicy::RecoveryV1 {
			summary: Some(summary),
			..
		}) => Some(&summary.model),
		_ => None,
	};
	if pinned != Some(reference) {
		return Err(Error::RemoteSemantic(Failure::Configuration));
	}
	let definition = inspection
		.definitions
		.iter()
		.find(|d| d.kind == "model" && &d.entry == reference)
		.ok_or(Error::RemoteSemantic(Failure::Configuration))?;
	Ok(Provider {
		node_id: node.to_owned(),
		entry: reference.clone(),
		digest: definition.digest.clone(),
		configuration_digest: digest(&definition.metadata.config),
	})
}
