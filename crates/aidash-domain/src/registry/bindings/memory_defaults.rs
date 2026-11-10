//! Canonical built-in memory definitions. The bank holds at most 64 live units;
//! retained identities and model operations are bounded to 4096 records each.
//! History and model results expire after 30 days, backups after seven days.
use super::*;
use crate::{context::source::*, memory::*};

pub const MEMORY_ID: &str = "aidash.memory";
pub const RERANKER_ID: &str = "aidash.memory-rrf";
pub const TOKENIZER_ID: &str = "aidash.memory-utf8";
pub const VERSION: &str = "1.0.0";

pub fn reference(node: &str) -> QualifiedRef {
	QualifiedRef {
		registry_deployment: node.into(),
		id: MEMORY_ID.into(),
		version: VERSION.into(),
	}
}

pub fn binding(node: &str) -> NormalizedBinding {
	NormalizedBinding {
		binding: super::Binding {
			kind: BindingKind::Memory,
			target: reference(node),
			alias: None,
			narrow: Narrowing::default(),
			members: vec![],
			exposure: None,
		},
		origin: BindingOrigin::Default,
	}
}

fn entry(id: &str, kind: &str, config: Value) -> Entry {
	Entry {
		binding_normalization: None,
		installation: None,
		id: id.into(),
		version: VERSION.into(),
		kind: kind.into(),
		name: [("en".into(), id.into())].into(),
		description: [("en".into(), "Built-in memory definition".into())].into(),
		capabilities: vec![],
		tags: vec!["system".into()],
		languages: vec![],
		skills: vec![],
		schema: serde_json::json!({}),
		config,
	}
}

pub fn definitions() -> Result<Vec<Entry>> {
	let rate = Rate {
		input_per_million: 1_000_000,
		output_per_million: 1_000_000,
	};
	let policy = Policy {
		extraction: RoleModel::AgentModel,
		derivation: RoleModel::AgentModel,
		reflection: RoleModel::AgentModel,
		embedding: None,
		reranker: EntityRef {
			id: RERANKER_ID.into(),
			version: VERSION.into(),
		},
		tokenizer: EntityRef {
			id: TOKENIZER_ID.into(),
			version: VERSION.into(),
		},
		semantic_link_min_similarity_millionths: 700_000,
		prices: Prices {
			extraction: rate,
			derivation: rate,
			reflection: rate,
			embedding: rate,
			reranker: rate,
		},
		retention: Retention {
			unit_max_age_days: None,
			candidate_days: 30,
			history_days: 30,
			history_versions: 8,
			model_result_days: 30,
			backup_days: 7,
			purge_after_seconds: 3600,
			purge_batch: 128,
			max_unit_records: 4096,
			max_model_operations: 4096,
		},
		decay: None,
		diversity_lambda_millionths: None,
		bounds: Bounds {
			max_unit_bytes: 8192,
			max_input_bytes: 16384,
			max_units: 64,
			max_candidates: 32,
			max_entities: 16,
			max_evidence: 16,
			max_links: 16,
			max_graph_hops: 3,
			max_graph_visits: 64,
			max_results: 8,
			max_context_tokens: 8192,
			max_model_calls: 4,
			max_model_tokens: 16384,
			max_cost_micros: 1_000_000,
			max_retries: 2,
			max_call_seconds: 30,
		},
		learn_from_runs: true,
		maintain_observations: true,
		refresh_mental_models: false,
	};
	policy.validate()?;
	let provider = ProviderConfig {
		engine: EngineKind::HindsightRust,
		policy,
		context_source: Some(SourceDeclaration {
			provider: "aidash.native-context/1".into(),
			operation: "semantic_memory".into(),
			tools: SourceTools::default(),
			pin: PinPolicy::OnDemand,
			volatility: Volatility::Run,
			query: QueryPolicy::TaskInputsV1,
			refresh: RefreshPolicy::RevisionBoundV1,
			disclosure: SourceDisclosure::default(),
			required_home: false,
		}),
	};
	Ok(vec![
		entry(
			RERANKER_ID,
			"reranker",
			serde_json::to_value(RerankerConfig::Rrf)?,
		),
		entry(
			TOKENIZER_ID,
			"tokenizer",
			serde_json::to_value(TokenizerConfig::Utf8UpperBound)?,
		),
		entry(MEMORY_ID, "memory", serde_json::to_value(provider)?),
	])
}
