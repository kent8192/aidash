use super::*;
use async_trait::async_trait;

fn policy() -> Policy {
	let model = EntityRef {
		id: "fixture".into(),
		version: "1.0.0".into(),
	};
	Policy {
		extraction: model.clone(),
		derivation: model.clone(),
		reflection: model.clone(),
		embedding: model.clone(),
		reranker: model.clone(),
		tokenizer: model,
		semantic_link_min_similarity_millionths: 700_000,
		decay: None,
		diversity_lambda_millionths: None,
		retention: Retention {
			unit_max_age_days: None,
			candidate_days: 7,
			history_days: 30,
			history_versions: 16,
			model_result_days: 7,
			backup_days: 7,
			purge_after_seconds: 60,
			purge_batch: 32,
			max_unit_records: 128,
			max_model_operations: 1024,
		},
		prices: Prices {
			extraction: Rate {
				input_per_million: 0,
				output_per_million: 0,
			},
			derivation: Rate {
				input_per_million: 0,
				output_per_million: 0,
			},
			reflection: Rate {
				input_per_million: 0,
				output_per_million: 0,
			},
			embedding: Rate {
				input_per_million: 0,
				output_per_million: 0,
			},
			reranker: Rate {
				input_per_million: 0,
				output_per_million: 0,
			},
		},
		bounds: Bounds {
			max_unit_bytes: 1024,
			max_input_bytes: 4096,
			max_units: 32,
			max_candidates: 8,
			max_entities: 4,
			max_evidence: 8,
			max_links: 4,
			max_graph_hops: 2,
			max_graph_visits: 8,
			max_results: 4,
			max_context_tokens: 4096,
			max_model_calls: 4,
			max_model_tokens: 8192,
			max_cost_micros: 1000,
			max_retries: 2,
			max_call_seconds: 30,
		},
		learn_from_runs: true,
		maintain_observations: false,
		refresh_mental_models: false,
	}
}
fn unit() -> Unit {
	Unit {
		id: Uuid::from_u128(1),
		bank: Bank {
			home: "home".into(),
			tenant: "tenant".into(),
			workspace: Uuid::from_u128(100),
			participant: Some(Uuid::from_u128(101)),
		},
		revision: 1,
		content: Content {
			mental_model: None,
			text: "Alice works in Tokyo / 東京".into(),
			kind: Kind::World,
			learning: Learning::Fact,
			verification: Verification::Unverified,
			occurred: None,
			entities: vec![],
			evidence: vec![],
			links: vec![],
		},
		learned_at: chrono::Utc::now(),
		updated_at: chrono::Utc::now(),
		deleted: false,
		stale: false,
	}
}
struct Scope {
	inclusive_reads: usize,
	retention_test: bool,
	stale: bool,
	foreign: bool,
	delivered: bool,
}
#[async_trait]
impl MemoryScope for Scope {
	async fn authorize(&mut self, _: &Bank, _: &EntityRef, _: &str) -> Result<()> {
		Ok(())
	}
	async fn snapshot(&mut self, _: &Bank, _: usize) -> Result<Snapshot> {
		assert_eq!(
			self.inclusive_reads, 0,
			"inclusive recall must use its atomic port"
		);
		Ok(Snapshot {
			units: if self.retention_test {
				let mut second = unit();
				second.id = Uuid::from_u128(2);
				vec![unit(), second]
			} else {
				vec![unit()]
			},
			graph: vec![],
			authority_revision: "1".into(),
		})
	}
	async fn recall_including_dormant_snapshot(&mut self, _: &Bank, _: usize) -> Result<Snapshot> {
		self.inclusive_reads += 1;
		let first = unit();
		let mut second = first.clone();
		second.id = Uuid::from_u128(2);
		Ok(Snapshot {
			units: vec![first, second],
			graph: vec![],
			authority_revision: "1".into(),
		})
	}
	async fn retention_scores(&mut self, _: &Bank, _: &[Unit]) -> Result<BTreeMap<Uuid, f64>> {
		assert!(self.retention_test);
		Ok(BTreeMap::from([
			(Uuid::from_u128(1), 0.1),
			(Uuid::from_u128(2), 1.0),
		]))
	}
	async fn current(&mut self, _: &Bank, _: &[Evidence]) -> Result<()> {
		if self.stale {
			Err(Error::Conflict("source changed".into()))
		} else {
			Ok(())
		}
	}
	async fn mutate(&mut self, _: &Mutation, _: &Bounds) -> Result<Vec<Unit>> {
		Err(Error::Forbidden)
	}
	async fn propose(
		&mut self,
		_: &Bank,
		_: &Evidence,
		_: &[Content],
		_: &Bounds,
	) -> Result<Vec<Candidate>> {
		Err(Error::Forbidden)
	}
	async fn review(
		&mut self,
		_: Uuid,
		_: i64,
		_: Option<&Mutation>,
		_: &Bounds,
	) -> Result<Option<Unit>> {
		Err(Error::Forbidden)
	}
	async fn publish(&mut self, _: &Evidence, _: &Mutation, _: &Bounds) -> Result<Vec<Unit>> {
		Err(Error::Forbidden)
	}
	async fn semantic(
		&mut self,
		_: &Bank,
		_: &EntityRef,
		_: &str,
		_: &[Uuid],
		_: usize,
		_: Allowance,
	) -> Result<Produced<Vec<Uuid>>> {
		Ok(Produced {
			output: if self.retention_test {
				vec![Uuid::from_u128(1), Uuid::from_u128(2)]
			} else {
				vec![if self.foreign {
					Uuid::from_u128(2)
				} else {
					unit().id
				}]
			},
			usage: Usage {
				tokens: 1,
				cost_micros: 1,
			},
		})
	}
	async fn keyword(&mut self, _: &Bank, _: &str, _: &[Uuid], _: usize) -> Result<Vec<Uuid>> {
		Ok(if self.retention_test {
			vec![Uuid::from_u128(2), Uuid::from_u128(1)]
		} else {
			vec![]
		})
	}
	async fn deliver(&mut self, bank: &Bank, _: &str, evidence: &[Evidence]) -> Result<()> {
		self.current(bank, evidence).await?;
		self.delivered = true;
		Ok(())
	}
}
struct Models {
	invented: bool,
	followup: Option<RecallQuery>,
}
#[async_trait]
impl MemoryModels for Models {
	fn reranker_uses_model(&self, model: &EntityRef) -> Result<bool> {
		Ok(model.id != "local-rrf")
	}
	async fn consolidate(
		&self,
		_: &EntityRef,
		mandatory: &[Unit],
		candidates: &[Unit],
		_: &Bounds,
		_: Allowance,
	) -> Result<Produced<Vec<Evidence>>> {
		Ok(Produced {
			output: if self.invented {
				vec![Evidence::Unit {
					bank: unit().bank,
					id: Uuid::from_u128(9),
					revision: 1,
				}]
			} else {
				mandatory
					.iter()
					.chain(candidates)
					.map(Unit::evidence)
					.collect()
			},
			usage: Usage {
				tokens: 1,
				cost_micros: 1,
			},
		})
	}
	async fn extract(
		&self,
		_: &EntityRef,
		_: &str,
		_: &[Evidence],
		_: extraction::Mode,
		_: &Bounds,
		_: Allowance,
	) -> Result<Produced<extraction::Extraction>> {
		Err(Error::Forbidden)
	}
	async fn derive(
		&self,
		_: &EntityRef,
		kind: Kind,
		mental_model: Option<&MentalModel>,
		units: &[Unit],
		_: &Bounds,
		_: Allowance,
	) -> Result<Produced<Content>> {
		let mut content = units[0].content.clone();
		content.kind = kind;
		content.mental_model = mental_model.cloned();
		content.evidence = units.iter().map(Unit::evidence).collect();
		if self.invented {
			content.verification = Verification::Supported;
		}
		Ok(Produced {
			output: content,
			usage: Usage {
				tokens: 1,
				cost_micros: 1,
			},
		})
	}
	async fn rerank(
		&self,
		model: &EntityRef,
		_: &str,
		units: &[Unit],
		allowance: Allowance,
	) -> Result<Produced<Vec<(Uuid, f64)>>> {
		let uses_model = self.reranker_uses_model(model)?;
		assert_eq!(allowance.calls > 0, uses_model);
		Ok(Produced {
			output: units.iter().map(|u| (u.id, 1.0)).collect(),
			usage: Usage {
				tokens: usize::from(uses_model || self.invented),
				cost_micros: u64::from(uses_model || self.invented),
			},
		})
	}
	async fn reflect(
		&self,
		_: &EntityRef,
		_: &str,
		context: &[Unit],
		_: &Bounds,
		_: Allowance,
	) -> Result<Produced<ReflectStep>> {
		Ok(Produced {
			output: if let Some(query) = &self.followup {
				ReflectStep::Recall {
					query: query.clone(),
				}
			} else {
				ReflectStep::Answer {
					reflection: Reflection {
						text: "Tokyo".into(),
						evidence: vec![if self.invented {
							Evidence::Unit {
								bank: unit().bank,
								id: Uuid::from_u128(9),
								revision: 1,
							}
						} else {
							context[0].evidence()
						}],
					},
				}
			},
			usage: Usage {
				tokens: 1,
				cost_micros: 1,
			},
		})
	}
	async fn tokens(&self, _: &EntityRef, envelope: &str) -> Result<usize> {
		Ok(envelope.len())
	}
}
fn query(tokens: usize) -> RecallQuery {
	RecallQuery {
		text: "where does Alice work?".into(),
		time: None,
		kinds: vec![],
		max_tokens: tokens,
	}
}

#[tokio::test]
async fn local_rrf_leaves_calls_for_embedding_and_reflection() {
	let models = Models {
		invented: false,
		followup: None,
	};
	for local in [false, true] {
		for reflection in [false, true] {
			let mut p = policy();
			p.bounds.max_model_calls = if reflection { 2 } else { 1 };
			if local {
				p.reranker.id = "local-rrf".into();
			}
			let engine = Engine {
				provider: &p.extraction,
				policy: &p,
				models: &models,
			};
			let mut scope = Scope {
				stale: false,
				foreign: false,
				delivered: false,
				retention_test: false,
				inclusive_reads: 0,
			};
			let result = if reflection {
				engine
					.reflect(&mut scope, &unit().bank, &query(4096))
					.await
					.map(|_| ())
			} else {
				engine
					.recall(&mut scope, &unit().bank, &query(4096))
					.await
					.map(|_| ())
			};
			assert_eq!(
				result.is_ok(),
				local,
				"local={local}, reflection={reflection}: {result:?}"
			);
		}
	}
}

#[tokio::test]
async fn a_local_reranker_cannot_report_unreserved_model_usage() {
	let mut p = policy();
	p.reranker.id = "local-rrf".into();
	let models = Models {
		invented: true,
		followup: None,
	};
	let engine = Engine {
		provider: &p.extraction,
		policy: &p,
		models: &models,
	};
	let mut scope = Scope {
		stale: false,
		foreign: false,
		delivered: false,
		retention_test: false,
		inclusive_reads: 0,
	};
	assert!(
		matches!(engine.recall(&mut scope, &unit().bank, &query(4096)).await,
		Err(Error::Invalid(message)) if message == "local reranker reported model usage")
	);
	assert!(!scope.delivered);
}

#[tokio::test]
async fn semantic_consolidation_shares_its_synthesis_budget_and_rechecks_sources() {
	let mut p = policy();
	p.bounds.max_model_calls = 3;
	let first = unit();
	let mut trigger = first.clone();
	trigger.id = Uuid::from_u128(2);
	trigger.content.text = "アリスは東京で働く".into();
	let snapshot = vec![first.clone(), trigger.clone()];
	let models = Models {
		invented: false,
		followup: None,
	};
	let engine = Engine {
		provider: &p.extraction,
		policy: &p,
		models: &models,
	};
	let mut scope = Scope {
		stale: false,
		foreign: false,
		delivered: false,
		retention_test: false,
		inclusive_reads: 0,
	};
	let result = engine
		.consolidate(&mut scope, &trigger, &snapshot)
		.await
		.unwrap();
	assert_eq!(result.units, snapshot);
	assert_eq!(result.allowance.calls, 1);
	assert_eq!(result.allowance.tokens, p.bounds.max_model_tokens - 2);
	assert_eq!(result.allowance.cost_micros, p.bounds.max_cost_micros - 2);
	scope.stale = true;
	assert!(matches!(
		engine.consolidate(&mut scope, &trigger, &snapshot).await,
		Err(Error::Conflict(_))
	));
	scope.stale = false;
	scope.foreign = true;
	assert!(matches!(
		engine.consolidate(&mut scope, &trigger, &snapshot).await,
		Err(Error::Invalid(_))
	));
	scope.foreign = false;
	let invented = Models {
		invented: true,
		followup: None,
	};
	let unsafe_engine = Engine {
		models: &invented,
		..engine
	};
	assert!(
		unsafe_engine
			.consolidate(&mut scope, &trigger, &snapshot)
			.await
			.is_err()
	);
	p.bounds.max_model_calls = 2;
	let exhausted = Engine {
		provider: &p.extraction,
		policy: &p,
		models: &models,
	};
	assert!(matches!(
		exhausted.consolidate(&mut scope, &trigger, &snapshot).await,
		Err(Error::Invalid(_))
	));
}
#[tokio::test]
async fn recall_refuses_foreign_candidates_and_stale_delivery() {
	let p = policy();
	let models = Models {
		invented: false,
		followup: None,
	};
	let engine = Engine {
		provider: &p.extraction,
		policy: &p,
		models: &models,
	};
	let mut scope = Scope {
		stale: false,
		foreign: true,
		delivered: false,
		retention_test: false,
		inclusive_reads: 0,
	};
	assert!(
		engine
			.recall(&mut scope, &unit().bank, &query(4096))
			.await
			.is_err()
	);
	assert!(!scope.delivered);
	scope.foreign = false;
	scope.stale = true;
	assert!(
		engine
			.recall(&mut scope, &unit().bank, &query(4096))
			.await
			.is_err()
	);
	assert!(!scope.delivered);
}
#[tokio::test]
async fn recall_counts_the_complete_envelope_and_distinguishes_no_space() {
	let p = policy();
	let models = Models {
		invented: false,
		followup: None,
	};
	let engine = Engine {
		provider: &p.extraction,
		policy: &p,
		models: &models,
	};
	let mut scope = Scope {
		stale: false,
		foreign: false,
		delivered: false,
		retention_test: false,
		inclusive_reads: 0,
	};
	assert_eq!(
		engine
			.recall(&mut scope, &unit().bank, &query(unit().content.text.len()))
			.await
			.unwrap(),
		Recall::NoSpace
	);
	assert!(matches!(
		engine
			.recall(&mut scope, &unit().bank, &query(4096))
			.await
			.unwrap(),
		Recall::Ready { .. }
	));
}
#[tokio::test]
async fn reflection_rejects_invented_citations() {
	let p = policy();
	let models = Models {
		invented: true,
		followup: None,
	};
	let engine = Engine {
		provider: &p.extraction,
		policy: &p,
		models: &models,
	};
	let mut scope = Scope {
		stale: false,
		foreign: false,
		delivered: false,
		retention_test: false,
		inclusive_reads: 0,
	};
	assert!(
		engine
			.reflect(&mut scope, &unit().bank, &query(4096))
			.await
			.is_err()
	);
}

#[tokio::test]
async fn maintenance_cannot_claim_verification_from_synthesis() {
	let p = policy();
	let source = unit();
	let mut draft = source.content.clone();
	draft.kind = Kind::Observation;
	draft.evidence = vec![source.evidence()];
	let mutation = Mutation {
		operation_id: Uuid::from_u128(20),
		provider: p.extraction.clone(),
		bank: source.bank.clone(),
		changes: vec![Change::Add {
			id: Uuid::from_u128(21),
			content: draft,
		}],
	};
	for invented in [false, true] {
		let models = Models {
			invented,
			followup: None,
		};
		let engine = Engine {
			provider: &p.extraction,
			policy: &p,
			models: &models,
		};
		let mut scope = Scope {
			stale: false,
			foreign: false,
			delivered: false,
			retention_test: false,
			inclusive_reads: 0,
		};
		let result = engine
			.maintain(
				&mut scope,
				&mutation,
				Kind::Observation,
				std::slice::from_ref(&source),
			)
			.await;
		if invented {
			assert!(
				matches!(
					result,
					Err(Error::Invalid(_) | Error::Domain(aidash_domain::Error::Invalid(_)))
				),
				"a model cannot verify its own derived claim"
			);
		} else {
			assert!(
				matches!(result, Err(Error::Forbidden)),
				"valid unverified synthesis reaches the adapter's admission decision"
			);
		}
	}
}

#[tokio::test]
async fn reflection_followup_cannot_expand_the_callers_context_budget() {
	let p = policy();
	let models = Models {
		invented: false,
		followup: Some(query(4096)),
	};
	let engine = Engine {
		provider: &p.extraction,
		policy: &p,
		models: &models,
	};
	let mut scope = Scope {
		stale: false,
		foreign: false,
		delivered: false,
		retention_test: false,
		inclusive_reads: 0,
	};
	assert!(
		matches!(engine.reflect(&mut scope, &unit().bank, &query(1024)).await,
		Err(Error::Invalid(message)) if message == "reflection context budget exhausted")
	);
}

#[tokio::test]
async fn model_reranker_ties_follow_retention_and_disabled_decay_keeps_id_order() {
	let provider = EntityRef {
		id: "fixture".into(),
		version: "1.0.0".into(),
	};
	let models = Models {
		invented: false,
		followup: None,
	};
	for enabled in [false, true] {
		let mut policy = policy();
		if enabled {
			policy.decay = Some(Decay {
				half_life_days: 30,
				prior_floor_millionths: 100_000,
				dormancy: None,
			});
		}
		let engine = Engine {
			provider: &provider,
			policy: &policy,
			models: &models,
		};
		let mut scope = Scope {
			stale: false,
			foreign: false,
			delivered: false,
			retention_test: true,
			inclusive_reads: 0,
		};
		let Recall::Ready { units } = engine
			.recall(&mut scope, &unit().bank, &query(4096))
			.await
			.unwrap()
		else {
			panic!("ready");
		};
		assert_eq!(
			units.iter().map(|u| u.id.as_u128()).collect::<Vec<_>>(),
			if enabled { vec![2, 1] } else { vec![1, 2] }
		);
		assert!(scope.delivered);
	}
}

#[tokio::test]
async fn inclusive_recall_uses_one_atomic_snapshot_instead_of_two_retention_reads() {
	let provider = EntityRef {
		id: "fixture".into(),
		version: "1.0.0".into(),
	};
	let policy = policy();
	let models = Models {
		invented: false,
		followup: None,
	};
	let engine = Engine {
		provider: &provider,
		policy: &policy,
		models: &models,
	};
	let mut scope = Scope {
		retention_test: false,
		inclusive_reads: 0,
		stale: false,
		foreign: false,
		delivered: false,
	};
	let result = engine
		.recall_including_dormant(&mut scope, &unit().bank, &query(4096))
		.await
		.unwrap();
	assert!(matches!(result, Recall::Ready { .. }));
	assert_eq!(scope.inclusive_reads, 1);
	assert!(scope.delivered);
}

/// Scope over a fixed snapshot with caller-chosen retrieval order and vectors.
struct DiverseScope {
	units: Vec<Unit>,
	vectors: BTreeMap<Uuid, Vec<f32>>,
	vector_reads: usize,
	delivered: Vec<Uuid>,
}
#[async_trait]
impl MemoryScope for DiverseScope {
	async fn authorize(&mut self, _: &Bank, _: &EntityRef, _: &str) -> Result<()> {
		Ok(())
	}
	async fn snapshot(&mut self, _: &Bank, _: usize) -> Result<Snapshot> {
		Ok(Snapshot {
			units: self.units.clone(),
			graph: vec![],
			authority_revision: "1".into(),
		})
	}
	async fn vectors(&mut self, _: &Bank, units: &[Unit]) -> Result<BTreeMap<Uuid, Vec<f32>>> {
		self.vector_reads += 1;
		assert!(
			units
				.iter()
				.all(|u| self.units.iter().any(|k| k.id == u.id))
		);
		Ok(self.vectors.clone())
	}
	async fn current(&mut self, _: &Bank, _: &[Evidence]) -> Result<()> {
		Ok(())
	}
	async fn mutate(&mut self, _: &Mutation, _: &Bounds) -> Result<Vec<Unit>> {
		Err(Error::Forbidden)
	}
	async fn propose(
		&mut self,
		_: &Bank,
		_: &Evidence,
		_: &[Content],
		_: &Bounds,
	) -> Result<Vec<Candidate>> {
		Err(Error::Forbidden)
	}
	async fn review(
		&mut self,
		_: Uuid,
		_: i64,
		_: Option<&Mutation>,
		_: &Bounds,
	) -> Result<Option<Unit>> {
		Err(Error::Forbidden)
	}
	async fn publish(&mut self, _: &Evidence, _: &Mutation, _: &Bounds) -> Result<Vec<Unit>> {
		Err(Error::Forbidden)
	}
	async fn semantic(
		&mut self,
		_: &Bank,
		_: &EntityRef,
		_: &str,
		allowed: &[Uuid],
		_: usize,
		_: Allowance,
	) -> Result<Produced<Vec<Uuid>>> {
		Ok(Produced {
			output: allowed.to_vec(),
			usage: Usage {
				tokens: 1,
				cost_micros: 1,
			},
		})
	}
	async fn keyword(&mut self, _: &Bank, _: &str, _: &[Uuid], _: usize) -> Result<Vec<Uuid>> {
		Ok(vec![])
	}
	async fn deliver(&mut self, _: &Bank, _: &str, evidence: &[Evidence]) -> Result<()> {
		self.delivered = evidence
			.iter()
			.map(|e| match e {
				Evidence::Unit { id, .. } => *id,
				_ => unreachable!("recall delivers Unit roots"),
			})
			.collect();
		Ok(())
	}
}
/// Reranks by a fixed table. Prices a delivery at ten tokens per Unit, or, when `exact`,
/// at the UTF-8 bytes of the envelope (the pinned `utf8_upper_bound` tokenizer).
struct ScoredModels(BTreeMap<Uuid, f64>, bool);
#[async_trait]
impl MemoryModels for ScoredModels {
	fn reranker_uses_model(&self, _: &EntityRef) -> Result<bool> {
		Ok(false)
	}
	async fn consolidate(
		&self,
		_: &EntityRef,
		_: &[Unit],
		_: &[Unit],
		_: &Bounds,
		_: Allowance,
	) -> Result<Produced<Vec<Evidence>>> {
		Err(Error::Forbidden)
	}
	async fn extract(
		&self,
		_: &EntityRef,
		_: &str,
		_: &[Evidence],
		_: extraction::Mode,
		_: &Bounds,
		_: Allowance,
	) -> Result<Produced<extraction::Extraction>> {
		Err(Error::Forbidden)
	}
	async fn derive(
		&self,
		_: &EntityRef,
		_: Kind,
		_: Option<&MentalModel>,
		_: &[Unit],
		_: &Bounds,
		_: Allowance,
	) -> Result<Produced<Content>> {
		Err(Error::Forbidden)
	}
	async fn rerank(
		&self,
		_: &EntityRef,
		_: &str,
		units: &[Unit],
		_: Allowance,
	) -> Result<Produced<Vec<(Uuid, f64)>>> {
		Ok(Produced {
			output: units.iter().map(|u| (u.id, self.0[&u.id])).collect(),
			usage: Usage {
				tokens: 0,
				cost_micros: 0,
			},
		})
	}
	async fn reflect(
		&self,
		_: &EntityRef,
		_: &str,
		_: &[Unit],
		_: &Bounds,
		_: Allowance,
	) -> Result<Produced<ReflectStep>> {
		Err(Error::Forbidden)
	}
	async fn tokens(&self, _: &EntityRef, envelope: &str) -> Result<usize> {
		Ok(if self.1 {
			envelope.len()
		} else {
			10 * envelope.matches("\"learned_at\"").count()
		})
	}
}
fn numbered(id: u128, text: &str) -> Unit {
	let mut unit = unit();
	unit.id = Uuid::from_u128(id);
	unit.content.text = text.into();
	unit
}
/// Run recall over `units` and return the delivered Unit numbers in delivery order.
async fn recalled(
	lambda: Option<u32>,
	units: Vec<Unit>,
	scores: &[(u128, f64)],
	vectors: &[(u128, [f32; 2])],
	tokens: usize,
) -> (Vec<u128>, DiverseScope) {
	recalled_priced(lambda, units, scores, vectors, tokens, false).await
}
async fn recalled_priced(
	lambda: Option<u32>,
	units: Vec<Unit>,
	scores: &[(u128, f64)],
	vectors: &[(u128, [f32; 2])],
	tokens: usize,
	exact: bool,
) -> (Vec<u128>, DiverseScope) {
	let mut p = policy();
	p.diversity_lambda_millionths = lambda;
	let models = ScoredModels(
		scores
			.iter()
			.map(|(id, score)| (Uuid::from_u128(*id), *score))
			.collect(),
		exact,
	);
	let engine = Engine {
		provider: &p.extraction,
		policy: &p,
		models: &models,
	};
	let mut scope = DiverseScope {
		units,
		vectors: vectors
			.iter()
			.map(|(id, v)| (Uuid::from_u128(*id), v.to_vec()))
			.collect(),
		vector_reads: 0,
		delivered: vec![],
	};
	let result = engine
		.recall(&mut scope, &unit().bank, &query(tokens))
		.await
		.unwrap();
	let Recall::Ready { units } = result else {
		panic!("expected delivered units, got {result:?}");
	};
	let order: Vec<_> = units.iter().map(|u| u.id.as_u128()).collect();
	assert_eq!(
		scope
			.delivered
			.iter()
			.map(|id| id.as_u128())
			.collect::<Vec<_>>(),
		order
	);
	(order, scope)
}
const PARAPHRASES: [(u128, f64); 4] = [(1, 1.0), (2, 0.95), (3, 0.9), (4, 0.8)];
const PARAPHRASE_VECTORS: [(u128, [f32; 2]); 4] = [
	(1, [1., 0.]),
	(2, [0.99, 0.01]),
	(3, [0.98, 0.02]),
	(4, [0., 1.]),
];
fn paraphrase_units() -> Vec<Unit> {
	vec![
		numbered(1, "Alice works in Tokyo"),
		numbered(2, "Alice is employed in Tokyo"),
		numbered(3, "Tokyo is where Alice works"),
		numbered(4, "Alice's manager is Bob"),
	]
}

#[tokio::test]
async fn diversity_delivers_the_distinct_unit_when_the_budget_fits_two() {
	let run = |lambda| {
		recalled(
			lambda,
			paraphrase_units(),
			&PARAPHRASES,
			&PARAPHRASE_VECTORS,
			20,
		)
	};
	let (without, untouched) = run(None).await;
	assert_eq!(
		without,
		[1, 2],
		"relevance order spends the budget on paraphrases"
	);
	assert_eq!(
		untouched.vector_reads, 0,
		"an unset policy never reads vectors"
	);
	let (with, _) = run(Some(700_000)).await;
	assert_eq!(with, [1, 4]);
	assert_eq!(run(Some(700_000)).await.0, with, "recall is deterministic");
}

#[tokio::test]
async fn diversity_never_adds_an_id_outside_the_authorized_candidates() {
	// The adapter returns a vector for an unauthorized Unit; it must not surface.
	let mut vectors = PARAPHRASE_VECTORS.to_vec();
	vectors.push((99, [0., 1.]));
	let (order, _) = recalled(
		Some(700_000),
		paraphrase_units(),
		&PARAPHRASES,
		&vectors,
		4096,
	)
	.await;
	assert_eq!(order, [1, 4, 2, 3]);
	assert!(!order.contains(&99));
}

#[tokio::test]
async fn equal_scores_and_similarities_order_by_unit_id() {
	let scores = [(1, 0.5), (2, 0.5), (3, 0.5)];
	let vectors = [(1, [1., 0.]), (2, [1., 0.]), (3, [0., 1.])];
	let mut shuffled = vec![numbered(3, "c"), numbered(1, "a"), numbered(2, "b")];
	let (first, _) = recalled(Some(700_000), shuffled.clone(), &scores, &vectors, 4096).await;
	shuffled.reverse();
	let (second, _) = recalled(Some(700_000), shuffled, &scores, &vectors, 4096).await;
	assert_eq!(first, [1, 3, 2]);
	assert_eq!(first, second);
}

#[tokio::test]
async fn an_observation_and_its_support_do_not_displace_a_non_redundant_unit() {
	let mut observation = numbered(10, "Alice works in Tokyo (summary)");
	observation.content.kind = Kind::Observation;
	observation.content.evidence = [1, 2]
		.map(|id| Evidence::Unit {
			bank: observation.bank.clone(),
			id: Uuid::from_u128(id),
			revision: 1,
		})
		.to_vec();
	let units = vec![
		observation,
		numbered(1, "Alice works in Tokyo"),
		numbered(2, "Alice is employed in Tokyo"),
		numbered(3, "Alice's manager is Bob"),
	];
	let scores = [(10, 1.0), (1, 0.9), (2, 0.8), (3, 0.5)];
	let vectors = [
		(10, [1., 0.]),
		(1, [0.6, 0.8]),
		(2, [0.8, 0.6]),
		(3, [0., 1.]),
	];
	let (baseline, _) = recalled(None, units.clone(), &scores, &vectors, 20).await;
	assert_eq!(baseline, [10, 1]);
	let (two, _) = recalled(Some(700_000), units.clone(), &scores, &vectors, 20).await;
	assert_eq!(
		two,
		[10, 3],
		"the summary and its sources are not co-delivered"
	);
	let (all, _) = recalled(Some(700_000), units, &scores, &vectors, 4096).await;
	assert_eq!(all, [10, 3, 1, 2], "sources follow when the budget allows");
}

#[tokio::test]
async fn candidates_without_a_current_vector_keep_relevance_order_instead_of_failing() {
	let (order, scope) = recalled(Some(700_000), paraphrase_units(), &PARAPHRASES, &[], 4096).await;
	assert_eq!(order, [1, 2, 3, 4]);
	assert_eq!(scope.vector_reads, 1);
	// A partial generation: only Unit 2 lacks a vector, so it is kept, not dropped.
	let partial: Vec<_> = PARAPHRASE_VECTORS
		.iter()
		.copied()
		.filter(|(id, _)| *id != 2)
		.collect();
	let (order, _) = recalled(
		Some(700_000),
		paraphrase_units(),
		&PARAPHRASES,
		&partial,
		4096,
	)
	.await;
	assert_eq!(order.len(), 4);
	assert!(order.contains(&2));
}

#[derive(serde::Deserialize)]
struct DiversityEvaluation {
	diversity_cases: Vec<DiversityCase>,
}
#[derive(serde::Deserialize)]
struct DiversityCase {
	id: String,
	language: String,
	lambda_millionths: u32,
	labels: Vec<String>,
	expected_useful_recall_without: f64,
	expected_useful_recall_with: f64,
	units: Vec<DiversityUnit>,
}
#[derive(serde::Deserialize)]
struct DiversityUnit {
	id: u128,
	text: String,
	label: String,
	score: f64,
	vector: Vec<f32>,
}

/// The frozen en/ja near-duplicate fixture through the real recall path, once without
/// and once with the pinned lambda, under a delivery budget that fits exactly two Units.
#[tokio::test]
async fn near_duplicate_fixture_reports_useful_recall_and_delivered_tokens_with_and_without_diversity()
 {
	let fixture: DiversityEvaluation = toml::from_str(include_str!(
		"../../../../tests/fixtures/native_memory_evaluation.toml"
	))
	.unwrap();
	assert_eq!(
		fixture
			.diversity_cases
			.iter()
			.map(|case| case.language.as_str())
			.collect::<Vec<_>>(),
		["en-US", "ja-JP"]
	);
	for case in fixture.diversity_cases {
		let units: Vec<_> = case
			.units
			.iter()
			.map(|row| {
				let mut unit = numbered(row.id, &row.text);
				// Fixed timestamps keep the serialized envelope, and so its size, stable.
				unit.learned_at = chrono::DateTime::UNIX_EPOCH;
				unit.updated_at = chrono::DateTime::UNIX_EPOCH;
				unit
			})
			.collect();
		let envelope = |subset: &[&Unit]| {
			serde_json::to_string(&Recall::Ready {
				units: subset.iter().map(|unit| (*unit).clone()).collect(),
			})
			.unwrap()
			.len()
		};
		let mut largest_pair = 0;
		let mut smallest_triple = usize::MAX;
		for (first, a) in units.iter().enumerate() {
			for (second, b) in units.iter().enumerate().skip(first + 1) {
				largest_pair = largest_pair.max(envelope(&[a, b]));
				for c in units.iter().skip(second + 1) {
					smallest_triple = smallest_triple.min(envelope(&[a, b, c]));
				}
			}
		}
		assert!(
			largest_pair < smallest_triple,
			"the budget must fit exactly two Units"
		);
		let scores: Vec<_> = case.units.iter().map(|row| (row.id, row.score)).collect();
		let vectors: Vec<_> =
			case.units
				.iter()
				.map(|row| {
					let vector: [f32; 2] = row.vector.clone().try_into().unwrap_or_else(|_| {
						panic!("{}: fixture vectors are two-dimensional", case.id)
					});
					(row.id, vector)
				})
				.collect();
		let mut measured = vec![];
		for lambda in [None, Some(case.lambda_millionths)] {
			let (order, _) =
				recalled_priced(lambda, units.clone(), &scores, &vectors, largest_pair, true).await;
			let delivered: Vec<&Unit> = order
				.iter()
				.map(|id| units.iter().find(|unit| unit.id.as_u128() == *id).unwrap())
				.collect();
			let useful =
				case.labels
					.iter()
					.filter(|label| {
						order.iter().any(|id| {
							case.units
								.iter()
								.any(|row| row.id == *id && &row.label == *label)
						})
					})
					.count() as f64 / case.labels.len() as f64;
			let tokens = envelope(&delivered);
			assert!(tokens <= largest_pair, "{}", case.id);
			measured.push((useful, tokens));
		}
		println!(
			"{} {}: useful recall {} -> {}, delivered tokens {} -> {}",
			case.id, case.language, measured[0].0, measured[1].0, measured[0].1, measured[1].1
		);
		assert_eq!(
			measured[0].0, case.expected_useful_recall_without,
			"{}",
			case.id
		);
		assert_eq!(
			measured[1].0, case.expected_useful_recall_with,
			"{}",
			case.id
		);
	}
}
