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
pub async fn binding<S: SemanticBindingScope + ?Sized>(
	access: &mut S,
	task: &Task,
	node: &str,
	inspection: &Inspection,
	request: &Request,
) -> Result<Binding> {
	let Request::RequiredHome {
		embedding,
		compactor,
	} = request
	else {
		return Ok(Binding::Disabled {});
	};
	if inspection.semantic_memory != VERSION || inspection.compactor.as_ref() != compactor.as_ref()
	{
		return Err(Error::RemoteSemantic(Failure::Configuration));
	}
	let agent: AgentConfig = AgentConfig::from_snapshot(&inspection.binding_snapshot)?;
	if !agent.semantic_memory && !agent.workspace_context {
		return Err(Error::RemoteSemantic(Failure::Configuration));
	}
	super::authorize(access, task, node, inspection).await?;
	let mut workspace = access.source_workspace(task.workspace_id).await?;
	workspace.attributes["remote_node"] = json!(node);
	workspace.attributes["inference_model"] = json!(agent.model);
	workspace.attributes["compactor"] = json!(compactor);
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
	if index.tenant != access.binding_tenant() || !spec.enabled || !spec.auto_context {
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
		home_lineage: access.binding_lineage().await?,
		execution_lineage: inspection.lineage.clone(),
		version: VERSION,
		index_revision: index.revision,
		index_digest: digest(&index.spec),
		embedding: Box::new(Provider {
			node_id: access.home_node_id().to_owned(),
			entry: embedding.clone(),
			digest: digest(&to_value(&entry)?),
			configuration_digest: digest(&entry.config),
		}),
		compactor: compactor.map(Box::new),
	})
}
