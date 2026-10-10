use super::*;
use aidash_domain::semantic::{EmbeddingConfig, Point, VectorConfig};
use async_trait::async_trait;
use chrono::Utc;
use rstest::rstest;
use serde_json::{Value, json};
use std::{
	collections::VecDeque,
	sync::{Arc, Mutex},
};

type Calls = Arc<Mutex<Vec<&'static str>>>;
fn spec() -> IndexingSpec {
	IndexingSpec {
		embedding: EmbeddingConfig {
			provider: "openai".into(),
			endpoint: "https://provider.invalid".into(),
			credential_env: None,
			provider_credential: None,
			model: "fixture".into(),
			model_version: "1".into(),
			dimensions: 2,
		},
		vector: VectorConfig {
			provider: "postgres".into(),
			endpoint: "https://vectors.invalid".into(),
			credential_env: None,
		},
		enabled: true,
		auto_context: true,
		max_sources: 16,
		max_results: 4,
		max_result_tokens: 4096,
		max_input_bytes: 128,
	}
}
fn index() -> Index {
	Index {
		workspace_id: Uuid::from_u128(1),
		tenant: "tenant".into(),
		revision: 4,
		spec: serde_json::to_value(spec()).unwrap(),
		collection: "collection".into(),
		updated_at: Utc::now(),
	}
}
fn input() -> Search {
	Search {
		query: "query".into(),
		agent: None,
		metadata: json!({}),
		limit: 4,
		max_tokens: 1024,
	}
}
fn entry() -> Entry {
	Entry {
		id: Uuid::from_u128(2),
		workspace_id: Uuid::from_u128(1),
		key: "key".into(),
		source: json!({"kind":"memory","text":"trusted content"}),
		agent: None,
		metadata: json!({"topic":"rust"}),
		revision: 1,
		point_id: Uuid::from_u128(3),
		index_revision: 4,
		deleted: false,
		state: "READY".into(),
		attempts: 0,
		last_error: None,
		created_by: "original".into(),
		updated_at: Utc::now(),
	}
}
fn point() -> Point {
	Point {
		id: entry().point_id,
		score: 0.5,
		payload: json!({"entry_id":entry().id,"revision":1,"index_revision":4,"tenant":"tenant","workspace_id":index().workspace_id,
            "text":"untrusted provider content", "metadata":{"topic":"provider"},"source":{"kind":"artifact","id":Uuid::from_u128(99)}}),
	}
}
struct Scope {
	calls: Calls,
	index: Index,
	rows: Vec<Entry>,
	permitted: VecDeque<bool>,
	texts: VecDeque<Option<String>>,
	digest: String,
	denied: bool,
	configured_exists: bool,
	embedded: Option<(Uuid, EmbeddingConfig, String, Option<Uuid>)>,
}
impl Scope {
	fn new(calls: Calls) -> Self {
		Self {
			calls,
			index: index(),
			rows: vec![entry()],
			permitted: VecDeque::new(),
			texts: VecDeque::new(),
			digest: content_digest("trusted content"),
			denied: false,
			configured_exists: true,
			embedded: None,
		}
	}
	fn touch(&self, name: &'static str) {
		self.calls.lock().unwrap().push(name);
	}
}
#[async_trait]
impl SemanticRetrievalSession for Scope {
	async fn workspace(&mut self, workspace: Uuid, action: &str) -> Result<()> {
		self.touch("workspace");
		assert_eq!(workspace, index().workspace_id);
		assert_eq!(action, "semantic.search");
		if self.denied {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
	async fn index(&mut self, workspace: Uuid) -> Result<Index> {
		self.touch("index");
		assert_eq!(workspace, index().workspace_id);
		Ok(self.index.clone())
	}
	async fn configured(&mut self, workspace: Uuid) -> Result<Option<Index>> {
		self.touch("configured");
		assert_eq!(workspace, index().workspace_id);
		Ok(self.configured_exists.then(|| self.index.clone()))
	}
	async fn candidates(&mut self, workspace: Uuid) -> Result<Vec<Entry>> {
		self.touch("candidates");
		assert_eq!(workspace, index().workspace_id);
		Ok(self.rows.clone())
	}
	async fn permits(&mut self, _: &Entry, action: &str) -> Result<bool> {
		self.touch("permits");
		assert_eq!(action, "semantic.read");
		Ok(self.permitted.pop_front().unwrap_or(true))
	}
	async fn source(&mut self, workspace: Uuid, _: &Source) -> Result<Option<String>> {
		self.touch("source");
		assert_eq!(workspace, index().workspace_id);
		Ok(self
			.texts
			.pop_front()
			.unwrap_or_else(|| Some("trusted content".into())))
	}
	async fn digest(&mut self, point: Uuid) -> Result<String> {
		self.touch("digest");
		assert_eq!(point, entry().point_id);
		Ok(self.digest.clone())
	}
	async fn embed(
		&mut self,
		workspace: Uuid,
		config: &EmbeddingConfig,
		text: &str,
		run: Option<Uuid>,
	) -> Result<Vec<f32>> {
		self.touch("embed");
		self.embedded = Some((workspace, config.clone(), text.into(), run));
		Ok(vec![0.25, 0.75])
	}
}
struct Vector {
	calls: Calls,
	points: Vec<Point>,
	available: Option<bool>,
	failure: Option<Failure>,
}
impl Vector {
	fn new(calls: Calls) -> Self {
		Self {
			calls,
			points: vec![point()],
			available: Some(true),
			failure: None,
		}
	}
}
#[async_trait]
impl VectorIndex for Vector {
	async fn ensure_collection(&self, _: &VectorConfig, _: &str, _: usize) -> Result<()> {
		panic!("retrieval must not create collections")
	}
	async fn upsert(&self, _: &VectorConfig, _: &str, _: Uuid, _: &[f32], _: Value) -> Result<()> {
		panic!("retrieval must not mutate points")
	}
	async fn delete_point(&self, _: &VectorConfig, _: &str, _: Uuid) -> Result<()> {
		panic!("retrieval must not delete points")
	}
	async fn delete_collection(&self, _: &VectorConfig, _: &str) -> Result<()> {
		panic!("retrieval must not delete collections")
	}
	async fn present(&self, config: &VectorConfig, collection: &str, ids: &[Uuid]) -> Result<bool> {
		self.calls.lock().unwrap().push("present");
		assert_eq!(config.provider, "postgres");
		assert_eq!(collection, "collection");
		assert_eq!(ids, &[entry().point_id]);
		self.available
			.ok_or_else(|| Error::External("provider failed".into()))
	}
	async fn query(
		&self,
		config: &VectorConfig,
		collection: &str,
		embedding: &[f32],
		filter: VectorFilter<'_>,
		limit: usize,
	) -> Result<Vec<Point>> {
		self.calls.lock().unwrap().push("query");
		assert_eq!(config.provider, "postgres");
		assert_eq!(collection, "collection");
		assert_eq!(embedding, [0.25, 0.75]);
		assert_eq!(filter.allowed, &[entry().point_id]);
		assert_eq!(filter.workspace, index().workspace_id);
		assert_eq!(filter.tenant, "tenant");
		assert_eq!(limit, 4);
		if let Some(failure) = self.failure {
			return Err(Error::RemoteSemantic(failure));
		}
		Ok(self
			.points
			.iter()
			.map(|point| serde_json::from_value(serde_json::to_value(point).unwrap()).unwrap())
			.collect())
	}
}
fn fixture() -> (Calls, Scope, Vector) {
	let calls = Calls::default();
	(calls.clone(), Scope::new(calls.clone()), Vector::new(calls))
}
fn controls(memory: bool, workspace: bool) -> AgentConfig {
	let mut controls: AgentConfig =
		serde_json::from_value(crate::test_support::agent("fixture").config).unwrap();
	controls.conversation_memory = memory;
	controls.semantic_memory = memory;
	controls.workspace_context = workspace;
	controls
}

#[rstest]
#[case::native_bank_only(false)]
#[case::explicit_workspace_source(true)]
#[tokio::test]
async fn native_memory_does_not_implicitly_enable_ordinary_workspace_search(
	#[case] workspace: bool,
) {
	let (calls, mut scope, vector) = fixture();
	scope.rows[0].source = json!({"kind":"artifact","id":Uuid::from_u128(90)});
	let mut agent = controls(true, workspace);
	agent.memory = Some(aidash_domain::registry::EntityRef {
		id: "native-memory".into(),
		version: "1.0.0".into(),
	});
	let result = context(&mut scope, &vector, &run(), "query", 1024, &agent)
		.await
		.unwrap();
	assert_eq!(result.is_some(), workspace);
	assert_eq!(scope.embedded.is_some(), workspace);
	if !workspace {
		assert!(calls.lock().unwrap().is_empty());
	}
}

#[rstest]
#[case::blank_query("query", json!("  "))]
#[case::query_byte_limit("query", json!("é".repeat(65)))]
#[case::zero_limit("limit", json!(0))]
#[case::too_many_results("limit", json!(5))]
#[case::zero_budget("max_tokens", json!(0))]
#[case::excessive_budget("max_tokens", json!(4097))]
#[case::nonobject_filter("metadata", json!([]))]
#[case::oversized_filter("metadata", json!({"x":"x".repeat(4096)}))]
#[tokio::test]
async fn invalid_input_stops_before_candidate_or_provider_reads(
	#[case] key: &str,
	#[case] value: Value,
) {
	// Arrange
	let (calls, mut scope, vector) = fixture();
	let mut request = input();
	match key {
		"query" => request.query = value.as_str().unwrap().into(),
		"limit" => request.limit = value.as_u64().unwrap() as usize,
		"max_tokens" => request.max_tokens = value.as_u64().unwrap() as usize,
		"metadata" => request.metadata = value,
		_ => panic!("unknown fixture field"),
	}
	// Act / Assert
	assert!(matches!(
		search(
			&mut scope,
			&vector,
			index().workspace_id,
			&request,
			None,
			None
		)
		.await,
		Err(Error::Domain(aidash_domain::Error::Invalid(_)))
	));
	assert_eq!(*calls.lock().unwrap(), ["workspace", "index"]);
}
#[rstest]
#[tokio::test]
async fn denied_workspace_stops_before_configuration_or_external_effects() {
	let (calls, mut scope, vector) = fixture();
	scope.denied = true;
	assert!(matches!(
		search(
			&mut scope,
			&vector,
			index().workspace_id,
			&input(),
			None,
			None
		)
		.await,
		Err(Error::Forbidden)
	));
	assert_eq!(*calls.lock().unwrap(), ["workspace"]);
}
#[rstest]
#[case::configuration(true)]
#[case::source(false)]
#[tokio::test]
async fn malformed_persisted_json_is_not_hidden_by_filters(#[case] configuration: bool) {
	let (calls, mut scope, vector) = fixture();
	if configuration {
		scope.index.spec = json!({});
	} else {
		scope.rows[0].source = json!({"kind":"unknown"});
	}
	let request = Search {
		agent: Some("another-agent".into()),
		metadata: json!({"topic":"other"}),
		..input()
	};
	assert!(matches!(
		search(
			&mut scope,
			&vector,
			index().workspace_id,
			&request,
			None,
			None
		)
		.await,
		Err(Error::Json(_))
	));
	assert_eq!(
		*calls.lock().unwrap(),
		if configuration {
			vec!["workspace", "index"]
		} else {
			vec!["workspace", "index", "candidates"]
		}
	);
}
#[rstest]
#[case::memory(json!({"kind":"memory","text":"private"}), false, true)]
#[case::artifact(json!({"kind":"artifact","id":Uuid::from_u128(6)}), true, false)]
#[case::message(json!({"kind":"message","id":Uuid::from_u128(7)}), true, false)]
#[tokio::test]
async fn agent_controls_filter_before_reading_source_content(
	#[case] source: Value,
	#[case] memory: bool,
	#[case] workspace: bool,
) {
	let (calls, mut scope, vector) = fixture();
	scope.rows[0].source = source;
	let result = search(
		&mut scope,
		&vector,
		index().workspace_id,
		&input(),
		None,
		Some(&controls(memory, workspace)),
	)
	.await
	.unwrap();
	assert_eq!(result.matches.len(), 0);
	assert_eq!(scope.embedded, None);
	assert_eq!(*calls.lock().unwrap(), ["workspace", "index", "candidates"]);
}
#[tokio::test]
async fn native_agent_never_unions_unconfigured_legacy_memory_sources() {
	let (calls, mut scope, vector) = fixture();
	scope.rows[0].source = json!({"kind":"memory","text":"unconfigured memory"});
	let mut agent = controls(true, true);
	agent.memory = Some(aidash_domain::registry::EntityRef {
		id: "native".into(),
		version: "1.0.0".into(),
	});
	let result = search(
		&mut scope,
		&vector,
		index().workspace_id,
		&input(),
		None,
		Some(&agent),
	)
	.await
	.unwrap();
	assert!(result.matches.is_empty());
	assert_eq!(scope.embedded, None);
	assert_eq!(*calls.lock().unwrap(), ["workspace", "index", "candidates"]);
}
#[rstest]
#[case::agent(0)]
#[case::metadata(1)]
#[case::authority(2)]
#[case::deleted_source(3)]
#[tokio::test]
async fn inaccessible_candidates_do_not_consume_provider_allowance(#[case] rejection: u8) {
	let (calls, mut scope, vector) = fixture();
	let mut request = input();
	match rejection {
		0 => scope.rows[0].agent = Some("scoped-agent".into()),
		1 => request.metadata = json!({"topic":"other"}),
		2 => scope.permitted.push_back(false),
		3 => scope.texts.push_back(None),
		_ => unreachable!(),
	}
	// An inaccessible pending entry cannot make another authority's search incomplete.
	scope.rows[0].state = "PENDING".into();
	let result = search(
		&mut scope,
		&vector,
		index().workspace_id,
		&request,
		None,
		None,
	)
	.await
	.unwrap();
	assert_eq!(result.matches.len(), 0);
	assert_eq!(scope.embedded, None);
	assert_eq!(
		calls
			.lock()
			.unwrap()
			.iter()
			.filter(|&&name| name == "present" || name == "embed" || name == "query")
			.count(),
		0
	);
}
#[rstest]
#[case::pending(0)]
#[case::stale_index(1)]
#[case::changed_source(2)]
#[tokio::test]
async fn incomplete_authorized_candidates_stop_before_provider_effects(#[case] incomplete: u8) {
	let (calls, mut scope, vector) = fixture();
	match incomplete {
		0 => scope.rows[0].state = "PENDING".into(),
		1 => scope.rows[0].index_revision -= 1,
		2 => scope.digest = content_digest("old content"),
		_ => unreachable!(),
	}
	let error = search(
		&mut scope,
		&vector,
		index().workspace_id,
		&input(),
		None,
		None,
	)
	.await
	.unwrap_err();
	assert_eq!(
		error.to_string(),
		"semantic index is incomplete; inspect entries and retry after indexing"
	);
	assert_eq!(
		*calls.lock().unwrap(),
		[
			"workspace",
			"index",
			"candidates",
			"permits",
			"source",
			"digest"
		]
	);
}
#[rstest]
#[case::missing(Some(false))]
#[case::provider_error(None)]
#[tokio::test]
async fn physical_point_check_precedes_embedding_allowance(#[case] available: Option<bool>) {
	let (calls, mut scope, mut vector) = fixture();
	vector.available = available;
	assert!(matches!(
		search(
			&mut scope,
			&vector,
			index().workspace_id,
			&input(),
			None,
			None
		)
		.await,
		Err(Error::SemanticUnavailable)
	));
	assert_eq!(scope.embedded, None);
	assert_eq!(calls.lock().unwrap().last(), Some(&"present"));
}
#[rstest]
#[tokio::test]
async fn authorized_search_uses_current_source_bytes_and_same_scope_for_embedding() {
	// Arrange
	let (calls, mut scope, vector) = fixture();
	let run = Uuid::from_u128(10);
	// Act
	let result = search(
		&mut scope,
		&vector,
		index().workspace_id,
		&input(),
		Some(run),
		None,
	)
	.await
	.unwrap();
	// Assert: provider payload cannot override locked source truth.
	assert_eq!(
		scope.embedded,
		Some((
			index().workspace_id,
			spec().embedding,
			"query".into(),
			Some(run)
		))
	);
	assert_eq!(result.matches.len(), 1);
	assert_eq!(result.matches[0].text, "trusted content");
	assert_eq!(result.matches[0].source, json!({"kind":"memory"}));
	assert_eq!(result.matches[0].metadata, json!({"topic":"rust"}));
	assert_eq!(result.estimated_tokens, result_tokens(&result).unwrap());
	assert!(!result.truncated);
	assert_eq!(
		*calls.lock().unwrap(),
		[
			"workspace",
			"index",
			"candidates",
			"permits",
			"source",
			"digest",
			"present",
			"embed",
			"query",
			"permits",
			"source"
		]
	);
}
#[rstest]
#[case::unknown_id("id")]
#[case::duplicate("duplicate")]
#[case::wrong_entry("entry_id")]
#[case::wrong_revision("revision")]
#[case::wrong_index("index_revision")]
#[case::wrong_tenant("tenant")]
#[case::wrong_workspace("workspace_id")]
#[tokio::test]
async fn strict_provider_contract_rejects_unapproved_or_mismatched_points(#[case] field: &str) {
	let (_, mut scope, mut vector) = fixture();
	match field {
		"id" => vector.points[0].id = Uuid::from_u128(99),
		"duplicate" => vector.points.push(point()),
		field => vector.points[0].payload[field] = json!("wrong"),
	}
	let prepared = prepare(&mut scope, index().workspace_id, &input(), None)
		.await
		.unwrap();
	assert!(matches!(
		finish(&vector, &mut scope, prepared, &[0.25, 0.75], true).await,
		Err(Error::RemoteSemantic(Failure::ProviderContract))
	));
}
#[rstest]
#[case::unknown_id("id", 0)]
#[case::duplicate("duplicate", 1)]
#[case::wrong_entry("entry_id", 0)]
#[case::wrong_revision("revision", 0)]
#[case::wrong_index("index_revision", 0)]
#[case::wrong_tenant("tenant", 0)]
#[case::wrong_workspace("workspace_id", 0)]
#[tokio::test]
async fn local_search_skips_invalid_provider_points(#[case] field: &str, #[case] expected: usize) {
	let (_, mut scope, mut vector) = fixture();
	match field {
		"id" => vector.points[0].id = Uuid::from_u128(99),
		"duplicate" => vector.points.push(point()),
		field => vector.points[0].payload[field] = json!("wrong"),
	}
	let result = search(
		&mut scope,
		&vector,
		index().workspace_id,
		&input(),
		None,
		None,
	)
	.await
	.unwrap();
	assert_eq!(result.matches.len(), expected);
}
#[rstest]
#[case::revoked(false, 0)]
#[case::removed(false, 1)]
#[case::changed(false, 2)]
#[case::strict_revoked(true, 0)]
#[case::strict_removed(true, 1)]
#[case::strict_changed(true, 2)]
#[tokio::test]
async fn source_and_authority_are_rechecked_at_delivery(
	#[case] strict: bool,
	#[case] rejection: u8,
) {
	let (_, mut scope, vector) = fixture();
	let prepared = prepare(&mut scope, index().workspace_id, &input(), None)
		.await
		.unwrap();
	match rejection {
		0 => scope.permitted.push_back(false),
		1 => scope.texts.push_back(None),
		2 => scope.texts.push_back(Some("changed".into())),
		_ => unreachable!(),
	}
	let result = finish(&vector, &mut scope, prepared, &[0.25, 0.75], strict).await;
	if strict {
		assert!(matches!(
			result,
			Err(Error::RemoteSemantic(Failure::Invalidated))
		));
	} else {
		assert_eq!(result.unwrap().matches.len(), 0);
	}
}
#[rstest]
#[case::local(false)]
#[case::strict(true)]
#[tokio::test]
async fn strict_search_preserves_safe_remote_provider_failure(#[case] strict: bool) {
	let (_, mut scope, mut vector) = fixture();
	vector.failure = Some(Failure::Allowance);
	let prepared = prepare(&mut scope, index().workspace_id, &input(), None)
		.await
		.unwrap();
	let result = finish(&vector, &mut scope, prepared, &[0.25, 0.75], strict).await;
	if strict {
		assert!(matches!(
			result,
			Err(Error::RemoteSemantic(Failure::Allowance))
		));
	} else {
		assert!(matches!(result, Err(Error::SemanticUnavailable)));
	}
}
#[rstest]
#[case::local(false)]
#[case::strict(true)]
#[tokio::test]
async fn strict_search_requires_backend_response_for_authorized_candidates(#[case] strict: bool) {
	let (_, mut scope, mut vector) = fixture();
	vector.points.clear();
	let prepared = prepare(&mut scope, index().workspace_id, &input(), None)
		.await
		.unwrap();
	let result = finish(&vector, &mut scope, prepared, &[0.25, 0.75], strict).await;
	if strict {
		assert!(matches!(result, Err(Error::SemanticUnavailable)));
	} else {
		assert_eq!(result.unwrap().matches.len(), 0);
	}
}
#[rstest]
#[tokio::test]
async fn token_budget_retains_empty_provenance_without_partial_match_content() {
	let (_, mut scope, vector) = fixture();
	let prepared = prepare(&mut scope, index().workspace_id, &input(), None)
		.await
		.unwrap();
	let budget = prepared.result.estimated_tokens;
	let request = Search {
		max_tokens: budget,
		..input()
	};
	let result = search(
		&mut scope,
		&vector,
		index().workspace_id,
		&request,
		None,
		None,
	)
	.await
	.unwrap();
	assert_eq!(result.matches.len(), 0);
	assert!(result.truncated);
	assert_eq!(result.estimated_tokens, budget);
	let request = Search {
		max_tokens: budget - 1,
		..input()
	};
	assert_eq!(
		prepare(&mut scope, index().workspace_id, &request, None)
			.await
			.err()
			.unwrap()
			.to_string(),
		"semantic token budget cannot hold provenance"
	);
}
#[rstest]
#[tokio::test]
async fn candidate_receipt_changes_with_source_or_entry_revision() {
	let (_, mut scope, _) = fixture();
	let initial = prepare(&mut scope, index().workspace_id, &input(), None)
		.await
		.unwrap()
		.candidate_digest();
	assert_eq!(
		prepare(&mut scope, index().workspace_id, &input(), None)
			.await
			.unwrap()
			.candidate_digest(),
		initial
	);
	scope.rows[0].revision += 1;
	assert_ne!(
		prepare(&mut scope, index().workspace_id, &input(), None)
			.await
			.unwrap()
			.candidate_digest(),
		initial
	);
	scope.rows[0].revision -= 1;
	scope.digest = content_digest("new content");
	scope.texts.push_back(Some("new content".into()));
	assert_ne!(
		prepare(&mut scope, index().workspace_id, &input(), None)
			.await
			.unwrap()
			.candidate_digest(),
		initial
	);
}
fn run() -> aidash_domain::Run {
	aidash_domain::Run {
		id: Uuid::from_u128(10),
		task_id: Uuid::from_u128(11),
		workspace_id: index().workspace_id,
		home_node: "aidash://home".into(),
		agent_id: "agent".into(),
		agent_version: "1".into(),
		state_version: aidash_domain::StateVersion::default(),
		state: aidash_domain::RunState::default(),
		recovery: aidash_domain::RecoveryState::default(),
		control: aidash_domain::RunControl::Active,
		context: aidash_domain::context::Context::default(),
		step: 1,
		revision: 1,
		observed_input_seq: 0,
		ledger_worker_ready: true,
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: Utc::now(),
	}
}
#[rstest]
#[case::no_index(0)]
#[case::disabled_index(1)]
#[case::manual_index(2)]
#[case::zero_budget(3)]
#[case::small_budget(4)]
#[case::controls_disabled(5)]
#[tokio::test]
async fn automatic_context_respects_configuration_and_provenance_budget_before_provider_io(
	#[case] reason: u8,
) {
	// Arrange
	let (calls, mut scope, vector) = fixture();
	let mut configuration = spec();
	let mut budget = 1024;
	match reason {
		0 => scope.configured_exists = false,
		1 => configuration.enabled = false,
		2 => configuration.auto_context = false,
		3 => budget = 0,
		4 => budget = 1,
		5 => {}
		_ => unreachable!(),
	}
	scope.index.spec = serde_json::to_value(configuration).unwrap();
	let agent = controls(reason != 5, reason != 5);
	// Act / Assert
	let result = context(&mut scope, &vector, &run(), "query", budget, &agent)
		.await
		.unwrap();
	assert!(result.is_none());
	assert_eq!(scope.embedded, None);
	assert_eq!(
		*calls.lock().unwrap(),
		if reason == 5 {
			vec![]
		} else {
			vec!["configured"]
		}
	);
}
#[rstest]
#[case::truncate_multibyte("éé", 3, "é")]
#[case::exact_byte_limit("éé", 4, "éé")]
#[case::ascii("abcdef", 3, "abc")]
#[tokio::test]
async fn automatic_context_truncates_query_at_utf8_boundary_and_keeps_run_origin(
	#[case] query: &str,
	#[case] max: usize,
	#[case] expected: &str,
) {
	let (calls, mut scope, vector) = fixture();
	let mut configuration = spec();
	configuration.max_input_bytes = max;
	configuration.max_result_tokens = 512;
	scope.index.spec = serde_json::to_value(configuration).unwrap();
	let agent =
		aidash_domain::qualified_agent(&run().home_node, &run().agent_id, &run().agent_version);
	scope.rows[0].agent = Some(agent.clone());
	let result = context(
		&mut scope,
		&vector,
		&run(),
		query,
		4096,
		&controls(true, true),
	)
	.await
	.unwrap()
	.unwrap();
	assert_eq!(result.matches[0].agent.as_deref(), Some(agent.as_str()));
	assert_eq!(
		scope.embedded,
		Some((
			index().workspace_id,
			spec().embedding,
			expected.into(),
			Some(run().id)
		))
	);
	assert!(result.estimated_tokens <= 512);
	assert_eq!(calls.lock().unwrap().first(), Some(&"configured"));
}
#[rstest]
#[tokio::test]
async fn malformed_automatic_context_configuration_retains_storage_error_identity() {
	let (calls, mut scope, vector) = fixture();
	scope.index.spec = json!({});
	assert!(matches!(
		context(
			&mut scope,
			&vector,
			&run(),
			"query",
			1024,
			&controls(true, true)
		)
		.await,
		Err(Error::Json(_))
	));
	assert_eq!(*calls.lock().unwrap(), ["configured"]);
}

#[tokio::test]
async fn disabled_auto_context_does_not_read_or_embed_ordinary_candidates() {
	let (calls, mut scope, vector) = fixture();
	let mut configuration = spec();
	configuration.auto_context = false;
	scope.index.spec = json!(configuration);
	let mut agent = controls(true, true);
	agent.memory = Some(aidash_domain::registry::EntityRef {
		id: "native".into(),
		version: "1.0.0".into(),
	});
	let result = search(
		&mut scope,
		&vector,
		index().workspace_id,
		&input(),
		None,
		Some(&agent),
	)
	.await
	.unwrap();
	assert!(result.matches.is_empty());
	assert_eq!(scope.embedded, None);
	assert_eq!(*calls.lock().unwrap(), ["workspace", "index"]);
}
