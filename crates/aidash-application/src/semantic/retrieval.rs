//! Deliver semantic candidates only after both initial and current source authority.
use crate::{
	Error, Result,
	ports::{VectorIndex, semantic::retrieval::SemanticRetrievalSession},
};
pub use aidash_domain::semantic::retrieval::result_tokens;
use aidash_domain::{
	registry::AgentConfig,
	semantic::{
		Failure, Source, VectorFilter,
		indexing::{IndexingSpec, content_digest},
		mutations::{Entry, Index},
		results::{Match, SearchResult},
		retrieval::{Search, provenance},
	},
};
use serde_json::json;
use std::collections::BTreeMap;
use uuid::Uuid;

pub struct PreparedSearch {
	pub index: Index,
	pub spec: IndexingSpec,
	pub allowed: BTreeMap<Uuid, (Entry, String)>,
	pub result: SearchResult,
	limit: usize,
	max_tokens: usize,
}
impl PreparedSearch {
	pub fn candidate_digest(&self) -> String {
		aidash_domain::registry::rules::digest(&json!(self.allowed.iter().map(|(point,(entry,text))|
            json!({"point":point,"entry":entry.id,"revision":entry.revision,"digest":content_digest(text),"agent":entry.agent}))
            .collect::<Vec<_>>()))
	}
	pub async fn check_points(&self, vector: &dyn VectorIndex) -> Result<()> {
		if self.allowed.is_empty() {
			return Ok(());
		}
		let Self {
			allowed,
			spec,
			index,
			..
		} = self;
		if !vector
			.present(
				&spec.vector,
				&index.collection,
				&allowed.keys().copied().collect::<Vec<_>>(),
			)
			.await
			.map_err(|_| Error::SemanticUnavailable)?
		{
			return Err(Error::SemanticUnavailable);
		}
		Ok(())
	}
}
pub async fn prepare(
	scope: &mut dyn SemanticRetrievalSession,
	workspace: Uuid,
	input: &Search,
	agent_controls: Option<&AgentConfig>,
) -> Result<PreparedSearch> {
	scope.workspace(workspace, "semantic.search").await?;
	let index = scope.index(workspace).await?;
	let spec = index.configuration()?;
	if !spec.enabled {
		return Err(Error::Conflict("semantic index is disabled".into()));
	}
	input.validate(&spec)?;
	// Agent context follows the index's ordinary disclosure switch.
	// Native memory is retrieved separately under its bank policy.
	let rows = if agent_controls.is_some() && !spec.auto_context {
		Vec::new()
	} else {
		scope.candidates(workspace).await?
	};
	if rows.len() > spec.max_sources {
		return Err(Error::Conflict("semantic source quota exceeded".into()));
	}
	let mut allowed = BTreeMap::new();
	let mut incomplete = false;
	for entry in rows {
		let source: Source = serde_json::from_value(entry.source.clone())?;
		if agent_controls.is_some_and(|agent| match &source {
			Source::Unit { .. } => true,
			Source::Memory { .. } => agent.memory.is_some() || !agent.semantic_memory,
			Source::Artifact { .. } | Source::Message { .. } => !agent.workspace_context,
		}) {
			continue;
		}
		if entry.agent.is_some() && entry.agent != input.agent {
			continue;
		}
		if !input
			.metadata
			.as_object()
			.expect("validated object")
			.iter()
			.all(|(k, v)| entry.metadata.get(k) == Some(v))
		{
			continue;
		}
		if !scope.permits(&entry, "semantic.read").await? {
			continue;
		}
		let Some(text) = scope.source(workspace, &source).await? else {
			continue;
		};
		// Source content must still match the bytes used for this point; linked
		// artifacts/messages can change outside this module's revision counter.
		let digest = scope.digest(entry.point_id).await?;
		if entry.state != "READY"
			|| entry.index_revision != index.revision
			|| digest != content_digest(&text)
		{
			incomplete = true;
			continue;
		}
		allowed.insert(entry.point_id, (entry, text));
	}
	if incomplete {
		return Err(Error::Conflict(
			"semantic index is incomplete; inspect entries and retry after indexing".into(),
		));
	}
	let mut result = SearchResult {
		workspace_id: workspace,
		index_revision: index.revision,
		model: spec.embedding.model.clone(),
		model_version: spec.embedding.model_version.clone(),
		matches: vec![],
		estimated_tokens: 0,
		truncated: false,
	};
	result.estimated_tokens = result_tokens(&result)?;
	if result.estimated_tokens > input.max_tokens {
		return Err(Error::Invalid(
			"semantic token budget cannot hold provenance".into(),
		));
	}
	Ok(PreparedSearch {
		index,
		spec,
		allowed,
		result,
		limit: input.limit,
		max_tokens: input.max_tokens,
	})
}
pub async fn finish(
	backend: &dyn VectorIndex,
	scope: &mut dyn SemanticRetrievalSession,
	prepared: PreparedSearch,
	vector: &[f32],
	strict: bool,
) -> Result<SearchResult> {
	let PreparedSearch {
		index,
		spec,
		allowed,
		mut result,
		limit,
		max_tokens,
	} = prepared;
	let workspace = index.workspace_id;
	let points = backend
		.query(
			&spec.vector,
			&index.collection,
			vector,
			VectorFilter {
				allowed: &allowed.keys().copied().collect::<Vec<_>>(),
				workspace,
				tenant: &index.tenant,
			},
			limit,
		)
		.await
		.map_err(|error| {
			if strict && matches!(error, Error::RemoteSemantic(_)) {
				error
			} else {
				Error::SemanticUnavailable
			}
		})?;
	if strict && !allowed.is_empty() && points.is_empty() {
		return Err(Error::SemanticUnavailable);
	}
	let mut seen = std::collections::BTreeSet::new();
	for point in points {
		let Some((entry, text)) = allowed.get(&point.id) else {
			if strict {
				return Err(Error::RemoteSemantic(Failure::ProviderContract));
			}
			continue;
		};
		if !seen.insert(point.id)
			|| point.payload["entry_id"] != entry.id.to_string()
			|| point.payload["revision"] != entry.revision
			|| point.payload["index_revision"] != index.revision
			|| point.payload["tenant"] != index.tenant
			|| point.payload["workspace_id"] != workspace.to_string()
		{
			if strict {
				return Err(Error::RemoteSemantic(Failure::ProviderContract));
			}
			continue;
		}
		// Check again at delivery while the authority and source row leases are
		// still held. Backend payload never supplies content or permission.
		if !scope.permits(entry, "semantic.read").await?
			|| scope
				.source(workspace, &serde_json::from_value(entry.source.clone())?)
				.await?
				.as_ref() != Some(text)
		{
			if strict {
				return Err(Error::RemoteSemantic(Failure::Invalidated));
			}
			continue;
		}
		let matched = Match {
			entry_id: entry.id,
			revision: entry.revision,
			source: provenance(&entry.source),
			agent: entry.agent.clone(),
			metadata: entry.metadata.clone(),
			text: text.clone(),
			score: point.score,
		};
		result.matches.push(matched);
		let tokens = result_tokens(&result)?;
		if tokens > max_tokens {
			result.matches.pop();
			result.truncated = true;
		} else {
			result.estimated_tokens = tokens;
		}
	}
	Ok(result)
}
pub async fn search(
	scope: &mut dyn SemanticRetrievalSession,
	vector: &dyn VectorIndex,
	workspace: Uuid,
	input: &Search,
	run: Option<Uuid>,
	controls: Option<&AgentConfig>,
) -> Result<SearchResult> {
	let prepared = prepare(scope, workspace, input, controls).await?;
	if prepared.allowed.is_empty() {
		return Ok(prepared.result);
	}
	prepared.check_points(vector).await?;
	let embedding = scope
		.embed(workspace, &prepared.spec.embedding, &input.query, run)
		.await?;
	finish(vector, scope, prepared, &embedding, false).await
}

pub async fn context(
	scope: &mut dyn SemanticRetrievalSession,
	vector: &dyn VectorIndex,
	run: &aidash_domain::Run,
	query: &str,
	budget: usize,
	agent: &AgentConfig,
) -> Result<Option<SearchResult>> {
	if !agent.semantic_memory && !agent.workspace_context {
		return Ok(None);
	}
	let configured = scope.configured(run.workspace_id).await?;
	let Some(configured) = configured else {
		return Ok(None);
	};
	let spec = configured.configuration()?;
	if !spec.enabled || !spec.auto_context {
		return Ok(None);
	}
	let minimum = SearchResult {
		workspace_id: run.workspace_id,
		index_revision: configured.revision,
		model: spec.embedding.model.clone(),
		model_version: spec.embedding.model_version.clone(),
		matches: vec![],
		estimated_tokens: 0,
		truncated: false,
	};
	if budget.min(spec.max_result_tokens) < result_tokens(&minimum)? {
		return Ok(None);
	}
	let mut query = query.to_owned();
	if query.len() > spec.max_input_bytes {
		let mut end = spec.max_input_bytes;
		while !query.is_char_boundary(end) {
			end -= 1;
		}
		query.truncate(end);
	}
	search(
		scope,
		vector,
		run.workspace_id,
		&Search {
			query,
			agent: Some(aidash_domain::qualified_agent(
				&run.home_node,
				&run.agent_id,
				&run.agent_version,
			)),
			metadata: json!({}),
			limit: spec.max_results,
			max_tokens: budget.min(spec.max_result_tokens),
		},
		Some(run.id),
		Some(agent),
	)
	.await
	.map(Some)
}

#[cfg(test)]
mod tests;

/// Reusing observed text still requires its current entry, source and index authority.
pub async fn recheck(
	scope: &mut dyn SemanticRetrievalSession,
	result: &SearchResult,
) -> Result<()> {
	scope
		.workspace(result.workspace_id, "semantic.search")
		.await?;
	let index = scope.index(result.workspace_id).await?;
	if index.revision != result.index_revision {
		return Err(Error::Conflict("observed semantic index changed".into()));
	}
	let entries = scope.candidates(result.workspace_id).await?;
	for matched in &result.matches {
		let entry = entries
			.iter()
			.find(|entry| {
				entry.id == matched.entry_id
					&& entry.revision == matched.revision
					&& provenance(&entry.source) == matched.source
			})
			.ok_or(Error::Forbidden)?;
		if !scope.permits(entry, "semantic.read").await? {
			return Err(Error::Forbidden);
		}
		if scope
			.source(
				result.workspace_id,
				&serde_json::from_value(entry.source.clone())?,
			)
			.await?
			.as_deref()
			!= Some(matched.text.as_str())
		{
			return Err(Error::Forbidden);
		}
	}
	Ok(())
}
