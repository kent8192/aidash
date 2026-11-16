//! Required Home retrieval binds current disclosure authority and exact provider/index definitions.
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
use serde_json::{json, to_value};
pub fn requirements(
	inspection: &Inspection,
) -> Result<aidash_domain::federation::execution::AgentMemoryRequirements> {
	let agent = AgentConfig::from_snapshot(&inspection.binding_snapshot)?;
	Ok(
		aidash_domain::federation::execution::AgentMemoryRequirements {
			native_required: agent.memory.as_ref().is_some_and(|memory| {
				memory.id != aidash_domain::registry::bindings::memory_defaults::MEMORY_ID
			}) && agent.allow_cross_conversation_memory != Some(false),
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
	let (embedding, native) = match request {
		Request::Disabled {} => {
			if native_enabled {
				return Err(Error::RemoteSemantic(Failure::Configuration));
			}
			return Ok(Binding::Disabled {});
		}
		Request::RequiredHome { embedding, native } => (embedding, native),
	};
	if native_enabled != native.is_some() {
		return Err(Error::RemoteSemantic(Failure::Configuration));
	}
	if inspection.semantic_memory != VERSION {
		return Err(Error::RemoteSemantic(Failure::Configuration));
	}
	if !agent.semantic_memory && !agent.workspace_context {
		return Err(Error::RemoteSemantic(Failure::Configuration));
	}
	super::authorize(access, task, node, inspection).await?;
	let subjects = sending_chain(access);
	let result = async {
		let mut workspace = access.source_workspace(task.workspace_id).await?;
		workspace.attributes["remote_node"] = json!(node);
		workspace.attributes["inference_model"] = json!(agent.model);
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
		})
	}
	.await;
	access.source_replace_subjects(subjects);
	result
}

fn sending_chain<S: SemanticBindingScope + ?Sized>(access: &mut S) -> Vec<String> {
	let subjects = access.source_subjects().to_vec();
	if let Some(executor) = subjects.last()
		&& !access.source_bundle().subjects.contains_key(executor)
	{
		access.source_replace_subjects(subjects[..subjects.len() - 1].to_vec());
	}
	subjects
}
