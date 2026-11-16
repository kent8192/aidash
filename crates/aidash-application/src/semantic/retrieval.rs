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
		degraded::OutageCause,
		indexing::{IndexingSpec, content_digest},
		mutations::{Entry, Index},
		results::{Match, SearchResult},
		retrieval::{Search, provenance},
	},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use uuid::Uuid;

pub struct PreparedSearch {
	pub index: Index,
	pub spec: IndexingSpec,
	pub allowed: BTreeMap<Uuid, (Entry, Candidate)>,
	pub result: SearchResult,
	limit: usize,
	max_tokens: usize,
	/// Some entries were not READY yet; `allowed` holds only the READY ones.
	/// Set only when the caller accepts a partial read (ADR 0129).
	incomplete: bool,
}
#[derive(Debug, Clone, PartialEq)]
pub enum Candidate {
	Text(String),
	Image(Value),
}
impl Candidate {
	fn digest(&self) -> String {
		match self {
			Self::Text(text) => content_digest(text),
			Self::Image(pointer) => pointer["digest"].as_str().unwrap_or_default().to_owned(),
		}
	}
}
/// Discard any adapter-supplied fields outside the File pointer contract.
fn image_pointer(source: &Source, pointer: Value) -> Option<Value> {
	let Source::WorkingImage { file_id, path, .. } = source else {
		return None;
	};
	if pointer["file_id"] != file_id.to_string() || pointer["path"] != *path {
		return None;
	}
	let name = pointer["name"].as_str()?;
	let media_type = pointer["media_type"].as_str()?;
	let size = pointer["size"].as_u64()?;
	let digest = pointer["digest"].as_str()?;
	if !media_type.starts_with("image/") || digest.is_empty() {
		return None;
	}
	Some(
		json!({"file_id":file_id,"path":path,"name":name,"media_type":media_type,"size":size,"digest":digest}),
	)
}
impl PreparedSearch {
	pub fn candidate_digest(&self) -> String {
		aidash_domain::registry::rules::digest(&json!(self.allowed.iter().map(|(point,(entry,content))|
            json!({"point":point,"entry":entry.id,"revision":entry.revision,"digest":content.digest(),"agent":entry.agent}))
            .collect::<Vec<_>>()))
	}
	pub async fn check_points(&self, vector: &dyn VectorIndex) -> Result<()> {
		if !self.points_present(vector).await? {
			// Indexed points the entries still require are missing from the
			// vector backend: an outage of that backend, retried by a Run.
			return Err(Error::RemoteSemantic(Failure::Unavailable));
		}
		Ok(())
	}
	/// Whether the vector backend holds every point the allowed entries need.
	/// A backend that cannot answer is `Unavailable`.
	async fn points_present(&self, vector: &dyn VectorIndex) -> Result<bool> {
		if self.allowed.is_empty() {
			return Ok(true);
		}
		let Self {
			allowed,
			spec,
			index,
			..
		} = self;
		vector
			.present(
				&spec.vector,
				&index.collection,
				&index.tenant,
				&allowed.keys().copied().collect::<Vec<_>>(),
			)
			.await
			.map_err(vector_outage)
	}
}
/// An unreachable or failing vector backend is a transient outage that a Run
/// retries (#233); a rejected query or invariant violation is not. A `Port`
/// error is untyped, so only the adapter can identify a temporary database
/// failure and preserve it for the Run's database recovery path.
fn vector_outage(error: Error) -> Error {
	match error {
		Error::TemporaryDatabase(_) => error,
		Error::RemoteSemantic(Failure::Unavailable) | Error::External(_) => {
			Error::RemoteSemantic(Failure::Unavailable)
		}
		_ => Error::SemanticUnavailable,
	}
}
pub async fn prepare(
	scope: &mut dyn SemanticRetrievalSession,
	workspace: Uuid,
	input: &Search,
	agent_controls: Option<&AgentConfig>,
) -> Result<PreparedSearch> {
	prepare_reads(scope, workspace, input, agent_controls, false).await
}

/// [`prepare`], optionally keeping the READY part of an incomplete index for a
/// Run step's optional context, which may degrade (ADR 0129).
async fn prepare_reads(
	scope: &mut dyn SemanticRetrievalSession,
	workspace: Uuid,
	input: &Search,
	agent_controls: Option<&AgentConfig>,
	partial: bool,
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
	if rows
		.iter()
		.filter(|entry| {
			entry.source.get("kind").and_then(|kind| kind.as_str()) != Some("working_image")
		})
		.count()
		> spec.max_sources
	{
		return Err(Error::Conflict("semantic source quota exceeded".into()));
	}
	let mut allowed = BTreeMap::new();
	let mut incomplete = false;
	for entry in rows {
		let source: Source = serde_json::from_value(entry.source.clone())?;
		if matches!(source, Source::WorkingImage { .. })
			&& (entry.state != "READY" || entry.index_revision != index.revision)
		{
			continue;
		}
		if agent_controls.is_some_and(|agent| match &source {
			Source::Unit { .. } => true,
			Source::Memory { .. } => bound_provider(agent) || !agent.semantic_memory,
			Source::Artifact { .. } | Source::Message { .. } | Source::WorkingImage { .. } => {
				!agent.workspace_context
			}
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
		let content = if matches!(source, Source::WorkingImage { .. }) {
			let Some(pointer) = scope
				.image_pointer(workspace, &source)
				.await?
				.and_then(|value| image_pointer(&source, value))
			else {
				continue;
			};
			Candidate::Image(pointer)
		} else {
			let Some(text) = scope.source(workspace, &source).await? else {
				continue;
			};
			Candidate::Text(text)
		};
		// Source content must still match the bytes used for this point; linked
		// artifacts/messages can change outside this module's revision counter.
		let digest = scope.digest(entry.point_id).await?;
		if entry.state != "READY"
			|| entry.index_revision != index.revision
			|| digest != content.digest()
		{
			if !matches!(content, Candidate::Image(_)) {
				incomplete = true;
			}
			continue;
		}
		allowed.insert(entry.point_id, (entry, content));
	}
	if incomplete && !(partial && agent_controls.is_some()) {
		// A Run waits for the complete index in an Outage Wait. Only a Run
		// step's optional context reads the READY part, and says so (ADR 0129).
		if agent_controls.is_some() {
			return Err(Error::RemoteSemantic(Failure::Unavailable));
		}
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
		incomplete,
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
		incomplete: _,
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
				vector_outage(error)
			}
		})?;
	if strict && !allowed.is_empty() && points.is_empty() {
		return Err(Error::SemanticUnavailable);
	}
	let mut seen = std::collections::BTreeSet::new();
	for point in points {
		let Some((entry, content)) = allowed.get(&point.id) else {
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
		let source: Source = serde_json::from_value(entry.source.clone())?;
		let current = if scope.permits(entry, "semantic.read").await? {
			match content {
				Candidate::Text(text) => {
					scope.source(workspace, &source).await?.as_ref() == Some(text)
				}
				Candidate::Image(pointer) => {
					scope
						.image_pointer(workspace, &source)
						.await?
						.and_then(|value| image_pointer(&source, value))
						.as_ref() == Some(pointer)
				}
			}
		} else {
			false
		};
		if !current {
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
			metadata: match content {
				Candidate::Text(_) => entry.metadata.clone(),
				Candidate::Image(pointer) => pointer.clone(),
			},
			text: match content {
				Candidate::Text(text) => text.clone(),
				Candidate::Image(_) => String::new(),
			},
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
	match search_or_outage(scope, vector, workspace, input, run, controls, false).await? {
		Retrieval::Found(result) => Ok(result),
		Retrieval::Incomplete(_) | Retrieval::Outage(_) => {
			Err(Error::RemoteSemantic(Failure::Unavailable))
		}
	}
}

/// What a search found: everything it asked for, only the READY part of an
/// incomplete index, or an outage that waiting can fix.
enum Retrieval {
	Found(SearchResult),
	Incomplete(SearchResult),
	Outage(OutageCause),
}

/// Search, returning an outage that waiting can fix as its cause instead of an
/// error (ADR 0059). Missing vector points stay an `Unavailable` error: the
/// index needs repair, so they never degrade.
async fn search_or_outage(
	scope: &mut dyn SemanticRetrievalSession,
	vector: &dyn VectorIndex,
	workspace: Uuid,
	input: &Search,
	run: Option<Uuid>,
	controls: Option<&AgentConfig>,
	partial: bool,
) -> Result<Retrieval> {
	let outage = |error: &Error| matches!(error, Error::RemoteSemantic(Failure::Unavailable));
	if let Some(image) = &input.image
		&& run != Some(image.run_id)
	{
		return Err(Error::Forbidden);
	}
	let prepared = prepare_reads(scope, workspace, input, controls, partial).await?;
	let found = if prepared.incomplete {
		Retrieval::Incomplete
	} else {
		Retrieval::Found
	};
	let query_image = if let Some(image) = &input.image {
		let content = scope
			.query_image(workspace, image, prepared.spec.max_image_bytes)
			.await?;
		aidash_domain::semantic::EmbeddingInput::Image {
			media_type: &content.media_type,
			bytes: &content.bytes,
		}
		.validate(&prepared.spec.embedding)?;
		Some(content)
	} else {
		None
	};
	if prepared.allowed.is_empty() {
		return Ok(found(prepared.result));
	}
	match prepared.points_present(vector).await {
		Ok(true) => {}
		Ok(false) => return Err(Error::RemoteSemantic(Failure::Unavailable)),
		Err(error) if outage(&error) => return Ok(Retrieval::Outage(OutageCause::VectorBackend)),
		Err(error) => return Err(error),
	}
	let embedding = match if let Some(image) = &query_image {
		scope
			.embed_image(
				workspace,
				&prepared.spec.embedding,
				image,
				Some(input.image.as_ref().expect("image query").run_id),
			)
			.await
	} else {
		scope
			.embed(workspace, &prepared.spec.embedding, &input.query, run)
			.await
	} {
		Err(error) if outage(&error) => {
			return Ok(Retrieval::Outage(OutageCause::EmbeddingProvider));
		}
		embedding => embedding?,
	};
	let max_image_bytes = prepared.spec.max_image_bytes;
	match finish(vector, scope, prepared, &embedding, false).await {
		Err(error) if outage(&error) => Ok(Retrieval::Outage(OutageCause::VectorBackend)),
		Err(error) => Err(error),
		Ok(result) => {
			if let (Some(selection), Some(original)) = (&input.image, &query_image) {
				let current = scope
					.query_image(workspace, selection, max_image_bytes)
					.await?;
				if current.media_type != original.media_type || current.bytes != original.bytes {
					return Err(Error::Conflict(
						"image query changed during retrieval".into(),
					));
				}
			}
			Ok(found(result))
		}
	}
}

/// The workspace slot of an Agent's semantic context.
#[derive(Debug)]
pub enum WorkspaceContext {
	Retrieved(Option<SearchResult>),
	/// The index is not complete yet: only its READY part was searched. The
	/// caller waits for the complete index unless the slot may degrade, in
	/// which case this part is its Degraded context (ADR 0129).
	Partial(SearchResult),
	/// Retrieval failed with an outage that waiting can fix. The caller waits
	/// unless the slot may degrade (ADR 0059, ADR 0129).
	Unavailable(OutageCause),
}
impl WorkspaceContext {
	/// The result, or the `Unavailable` error a Run retries and then pauses on.
	pub fn required(self) -> Result<Option<SearchResult>> {
		match self {
			Self::Retrieved(result) => Ok(result),
			Self::Partial(_) | Self::Unavailable(_) => {
				Err(Error::RemoteSemantic(Failure::Unavailable))
			}
		}
	}
}

pub async fn context(
	scope: &mut dyn SemanticRetrievalSession,
	vector: &dyn VectorIndex,
	run: &aidash_domain::Run,
	query: &str,
	budget: usize,
	agent: &AgentConfig,
) -> Result<Option<SearchResult>> {
	workspace_context(scope, vector, run, query, budget, agent)
		.await?
		.required()
}

pub async fn workspace_context(
	scope: &mut dyn SemanticRetrievalSession,
	vector: &dyn VectorIndex,
	run: &aidash_domain::Run,
	query: &str,
	budget: usize,
	agent: &AgentConfig,
) -> Result<WorkspaceContext> {
	// A native provider opts into its own Bank retrieval. Ordinary workspace
	// indexing remains available only through an explicit workspace Source.
	if (!agent.semantic_memory || bound_provider(agent)) && !agent.workspace_context {
		return Ok(WorkspaceContext::Retrieved(None));
	}
	let configured = scope.configured(run.workspace_id).await?;
	let Some(configured) = configured else {
		return Ok(WorkspaceContext::Retrieved(None));
	};
	let spec = configured.configuration()?;
	if !spec.enabled || !spec.auto_context {
		return Ok(WorkspaceContext::Retrieved(None));
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
		return Ok(WorkspaceContext::Retrieved(None));
	}
	let mut query = query.to_owned();
	if query.len() > spec.max_input_bytes {
		let mut end = spec.max_input_bytes;
		while !query.is_char_boundary(end) {
			end -= 1;
		}
		query.truncate(end);
	}
	let result = search_or_outage(
		scope,
		vector,
		run.workspace_id,
		&Search {
			query,
			image: None,
			agent: Some(aidash_domain::agent_subject(
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
		true,
	)
	.await?;
	Ok(match result {
		Retrieval::Found(result) => WorkspaceContext::Retrieved(Some(result)),
		Retrieval::Incomplete(result) => WorkspaceContext::Partial(result),
		Retrieval::Outage(cause) => WorkspaceContext::Unavailable(cause),
	})
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
		let source: Source = serde_json::from_value(entry.source.clone())?;
		let current = if matches!(source, Source::WorkingImage { .. }) {
			matched.text.is_empty()
				&& scope
					.image_pointer(result.workspace_id, &source)
					.await?
					.and_then(|value| image_pointer(&source, value))
					.as_ref() == Some(&matched.metadata)
		} else {
			scope.source(result.workspace_id, &source).await?.as_deref()
				== Some(matched.text.as_str())
		};
		if !current {
			return Err(Error::Forbidden);
		}
	}
	Ok(())
}

/// Whether the Agent binds its own memory provider. Built-in Memory recalls
/// once at Run start (#852), so it does not replace a native semantic memory
/// Source in per-step retrieval.
fn bound_provider(agent: &AgentConfig) -> bool {
	agent.memory.as_ref().is_some_and(|memory| {
		memory.id != aidash_domain::registry::bindings::memory_defaults::MEMORY_ID
	})
}
