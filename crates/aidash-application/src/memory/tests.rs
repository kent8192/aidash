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
