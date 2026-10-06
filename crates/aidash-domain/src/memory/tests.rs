use super::*;
use crate::memory::recall::{Rankings, fuse, graph, temporal};
use std::collections::BTreeSet;

fn bounds() -> Bounds {
	Bounds {
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
	}
}
fn unit(id: u128) -> Unit {
	Unit {
		id: Uuid::from_u128(id),
		bank: Bank {
			home: "home".into(),
			tenant: "tenant".into(),
			workspace: Uuid::from_u128(100),
			participant: Some(Uuid::from_u128(101)),
		},
		revision: 1,
		content: Content {
			mental_model: None,
			text: "日本語で回答する".into(),
			kind: Kind::World,
			learning: Learning::Preference,
			verification: Verification::Unverified,
			occurred: None,
			entities: vec![],
			evidence: vec![],
			links: vec![],
		},
		learned_at: Utc::now(),
		updated_at: Utc::now(),
		deleted: false,
		stale: false,
	}
}
#[test]
fn recovery_fences_never_lower_revisions_or_revive_deleted_identities() {
	let mut ledger = recovery::Ledger::new("home".into());
	let first = unit(1);
	assert!(!ledger.current(&first).unwrap());
	ledger.observe(&first).unwrap();
	assert!(ledger.current(&first).unwrap());
	let mut corrected = first.clone();
	corrected.revision = 2;
	corrected.content.text = "Corrected".into();
	ledger.observe(&corrected).unwrap();
	assert!(!ledger.current(&first).unwrap());
	assert!(ledger.observe(&first).is_err());
	assert!(ledger.current(&corrected).unwrap());
	let mut deleted = corrected.clone();
	deleted.revision = 3;
	deleted.deleted = true;
	deleted.content.text.clear();
	ledger.observe(&deleted).unwrap();
	assert!(!ledger.current(&corrected).unwrap());
	corrected.revision = 4;
	assert!(ledger.observe(&corrected).is_err());
	let mut other = unit(1);
	other.bank.workspace = Uuid::new_v4();
	other.revision = 4;
	other.deleted = true;
	assert!(ledger.observe(&other).is_err());
	assert!(ledger.validate("another-home").is_err());
}
#[test]
fn recovery_fences_learning_time_at_database_precision() {
	let first = unit(1);
	let mut ledger = recovery::Ledger::new("home".into());
	ledger.observe(&first).unwrap();
	let mut database = first.clone();
	database.learned_at =
		DateTime::from_timestamp_micros(first.learned_at.timestamp_micros()).unwrap();
	assert!(ledger.current(&database).unwrap());
	database.learned_at += chrono::Duration::days(1);
	assert!(
		!ledger.current(&database).unwrap(),
		"a rewind cannot reset the retention clock"
	);
}
#[test]
fn uncertain_positive_fences_reject_same_revision_identity_reuse() {
	let first = unit(1);
	let mut ledger = recovery::Ledger::new("home".into());
	ledger.observe(&first).unwrap();
	ledger.observe(&first).unwrap();
	let mut replacement = first.clone();
	replacement.content.text = "A different request cannot replace the uncertain revision".into();
	assert!(ledger.observe(&replacement).is_err());
	assert!(ledger.current(&first).unwrap());
	let mut withdrawn = first.clone();
	withdrawn.stale = true;
	withdrawn.content.text.clear();
	ledger.observe(&withdrawn).unwrap();
	assert!(!ledger.current(&first).unwrap());
	assert!(
		ledger.observe(&first).is_err(),
		"clearing a revision cannot reopen its old body"
	);
}
#[test]
fn logical_retention_expires_at_the_declared_boundary() {
	let retention = Retention {
		unit_max_age_days: Some(1),
		candidate_days: 7,
		history_days: 30,
		history_versions: 16,
		model_result_days: 7,
		backup_days: 7,
		purge_after_seconds: 60,
		purge_batch: 32,
		max_unit_records: 128,
		max_model_operations: 1024,
	};
	let learned = unit(1).learned_at;
	let expires = learned + chrono::Duration::days(1);
	assert!(!retention.unit_expired(learned, expires - chrono::Duration::microseconds(1)));
	assert!(retention.unit_expired(learned, expires));
	assert!(retention.unit_expired(learned, expires + chrono::Duration::seconds(1)));
	let unlimited = Retention {
		unit_max_age_days: None,
		..retention
	};
	assert!(!unlimited.unit_expired(learned, expires));
}
#[test]
fn fusion_deduplicates_each_arm_and_rejects_unscoped_hits() {
	let a = Uuid::from_u128(1);
	let b = Uuid::from_u128(2);
	let allowed = BTreeSet::from([a, b]);
	let ranks = Rankings {
		semantic: vec![a, a, b],
		keyword: vec![b, a],
		..Rankings::default()
	};
	let result = fuse(&ranks, &allowed, 8).unwrap();
	assert_eq!(result[0].id, a);
	assert_eq!(result[0].score, result[1].score);
	assert!(fuse(&ranks, &BTreeSet::from([a]), 8).is_err());
}
#[test]
fn graph_does_not_cross_authority_or_revision_fences() {
	let mut a = unit(1);
	let mut b = unit(2);
	let c = unit(3);
	a.content.links = vec![
		Link {
			target: b.id,
			revision: 1,
			kind: LinkKind::Causes,
			weight: 1.0,
		},
		Link {
			target: c.id,
			revision: 1,
			kind: LinkKind::Entity,
			weight: 1.0,
		},
	];
	assert_eq!(
		graph(&[a.clone(), b.clone()], &[a.id], &bounds()),
		vec![b.id]
	);
	b.revision = 2;
	assert!(graph(&[a.clone(), b], &[a.id], &bounds()).is_empty());
}

#[test]
fn semantic_graph_uses_current_finite_vectors_and_bounded_same_bank_edges() {
	use std::collections::BTreeMap;
	let a = unit(1);
	let mut b = unit(2);
	b.revision = 4;
	let c = unit(3);
	let mut foreign = unit(4);
	foreign.bank.participant = Some(Uuid::from_u128(999));
	let snapshot = vec![a.clone(), b.clone(), c.clone(), foreign];
	let vectors = BTreeMap::from([
		(a.id, vec![1., 0.]),
		(b.id, vec![1., 0.]),
		(c.id, vec![0., 1.]),
		(Uuid::from_u128(4), vec![1., 0.]),
	]);
	let mut settings = bounds();
	settings.max_graph_hops = 1;
	let edges = super::graph::semantic(&snapshot, &vectors, 1_000_000, &bounds()).unwrap();
	assert_eq!(edges.len(), 2);
	assert_eq!(edges[0].source, a.id);
	assert_eq!(edges[0].source_revision, a.revision);
	assert_eq!(edges[0].target.target, b.id);
	assert_eq!(edges[0].target.revision, b.revision);
	assert_eq!(edges[0].target.kind, LinkKind::Semantic);
	assert_eq!(
		recall::graph_with_edges(&snapshot, &[a.id], &edges, &settings),
		vec![b.id]
	);
	let mut changed = snapshot.clone();
	changed[1].revision += 1;
	assert!(recall::graph_with_edges(&changed, &[a.id], &edges, &bounds()).is_empty());
	changed = snapshot.clone();
	changed[0].revision += 1;
	assert!(recall::graph_with_edges(&changed, &[a.id], &edges, &bounds()).is_empty());
	let mut invalid = vectors.clone();
	invalid.insert(a.id, vec![f32::NAN, 0.]);
	assert!(super::graph::semantic(&snapshot, &invalid, 700_000, &bounds()).is_err());
	invalid.insert(a.id, vec![0., 0.]);
	assert!(super::graph::semantic(&snapshot, &invalid, 700_000, &bounds()).is_err());
	invalid.insert(a.id, vec![1.]);
	assert!(super::graph::semantic(&snapshot, &invalid, 700_000, &bounds()).is_err());
	assert!(super::graph::semantic(&[a.clone(), a], &vectors, 700_000, &bounds()).is_err());
	assert!(super::graph::semantic(&snapshot, &vectors, 0, &bounds()).is_err());
	let mut small = bounds();
	small.max_links = 1;
	let dense: BTreeMap<_, _> = snapshot[..3]
		.iter()
		.map(|unit| (unit.id, vec![1., 0.1]))
		.collect();
	assert_eq!(
		super::graph::semantic(&snapshot[..3], &dense, 700_000, &small)
			.unwrap()
			.len(),
		3
	);
}

#[test]
fn causal_extraction_binds_only_admitted_batch_ids_and_keeps_claims_unverified() {
	use extraction::{CausalRelation, Extraction};
	let first = unit(1);
	let mut second = unit(2);
	second.content.text = "雨で列車が遅れた。 The train was delayed by rain.".into();
	let batch = || Extraction {
		facts: vec![first.content.clone(), second.content.clone()],
		causal: vec![CausalRelation {
			cause: 0,
			effect: 1,
			weight: 0.8,
		}],
	};
	let result = batch().resolve(&[first.id, second.id], &bounds()).unwrap();
	assert_eq!(
		result[0].links[0],
		Link {
			target: second.id,
			revision: 1,
			kind: LinkKind::Causes,
			weight: 0.8
		}
	);
	assert_eq!(
		result[1].links[0],
		Link {
			target: first.id,
			revision: 1,
			kind: LinkKind::CausedBy,
			weight: 0.8
		}
	);
	assert_eq!(result[1].verification, Verification::Unverified);
	assert_eq!(result[1].text, second.content.text);
	for (cause, effect, weight) in [
		(0, 2, 1.),
		(0, 0, 1.),
		(0, 1, f64::NAN),
		(0, 1, 0.),
		(0, 1, 1.1),
	] {
		let mut malformed = batch();
		malformed.causal = vec![CausalRelation {
			cause,
			effect,
			weight,
		}];
		assert!(
			malformed
				.resolve(&[first.id, second.id], &bounds())
				.is_err()
		);
	}
	let mut repeated = batch();
	repeated.causal.push(repeated.causal[0].clone());
	assert!(repeated.resolve(&[first.id, second.id], &bounds()).is_err());
	assert!(batch().resolve(&[first.id, first.id], &bounds()).is_err());
	assert!(
		batch()
			.resolve(&[Uuid::nil(), second.id], &bounds())
			.is_err()
	);
	assert!(batch().resolve(&[first.id], &bounds()).is_err());
	let mut small = bounds();
	small.max_links = 0;
	assert!(batch().resolve(&[first.id, second.id], &small).is_err());
}

#[test]
fn semantic_consolidation_cannot_drop_conflicts_or_add_foreign_changed_support() {
	let first = unit(1);
	let mut conflict = unit(2);
	conflict.content.text = "日本語では回答しない".into();
	let candidate = unit(3);
	let mandatory = vec![first.clone(), conflict.clone()];
	let selected = vec![first.evidence(), conflict.evidence(), candidate.evidence()];
	assert_eq!(
		consolidation::select(
			&mandatory,
			std::slice::from_ref(&candidate),
			&selected,
			&bounds()
		)
		.unwrap(),
		vec![first.clone(), conflict.clone(), candidate.clone()]
	);
	assert!(
		consolidation::select(
			&mandatory,
			std::slice::from_ref(&candidate),
			&selected[..1],
			&bounds()
		)
		.is_err()
	);
	let mut changed = selected.clone();
	changed[2] = Evidence::Unit {
		bank: candidate.bank.clone(),
		id: candidate.id,
		revision: 2,
	};
	assert!(
		consolidation::select(
			&mandatory,
			std::slice::from_ref(&candidate),
			&changed,
			&bounds()
		)
		.is_err()
	);
	let mut repeated = selected.clone();
	repeated.push(selected[0].clone());
	assert!(
		consolidation::select(
			&mandatory,
			std::slice::from_ref(&candidate),
			&repeated,
			&bounds()
		)
		.is_err()
	);
	for foreign in [
		{
			let mut unit = candidate.clone();
			unit.bank.participant = None;
			unit
		},
		{
			let mut unit = candidate.clone();
			unit.content.kind = Kind::Experience;
			unit
		},
		{
			let mut unit = candidate.clone();
			unit.content.learning = Learning::Procedure;
			unit
		},
		{
			let mut unit = candidate.clone();
			unit.stale = true;
			unit
		},
	] {
		assert!(consolidation::select(&mandatory, &[foreign], &selected, &bounds()).is_err());
	}
	let mut small = bounds();
	small.max_evidence = 2;
	assert!(consolidation::select(&mandatory, &[candidate], &selected, &small).is_err());
}

#[test]
fn extending_a_semantic_observation_preserves_previous_conflicts_transitively() {
	let a = unit(1);
	let mut b = unit(2);
	b.content.text = "Do not answer in Japanese.".into();
	let c = unit(3);
	let mut previous = unit(4);
	previous.content.kind = Kind::Observation;
	previous.content.evidence = vec![a.evidence(), b.evidence()];
	let snapshot = vec![a.clone(), b.clone(), c.clone(), previous];
	let result = consolidation::preserve_observations(&[a, c], &snapshot, &bounds()).unwrap();
	assert_eq!(result, snapshot[..3]);
	let mut changed = snapshot.clone();
	changed[1].revision += 1;
	assert!(consolidation::preserve_observations(&result[..1], &changed, &bounds()).is_err());
	let mut small = bounds();
	small.max_evidence = 1;
	assert!(consolidation::preserve_observations(&result[..1], &snapshot, &small).is_err());
}
#[test]
fn temporal_uses_occurrence_and_never_invents_it_from_learned_time() {
	let mut a = unit(1);
	let b = unit(2);
	let window = TimeRange {
		start: a.learned_at,
		end: a.learned_at,
	};
	assert!(temporal(&[a.clone(), b.clone()], Some(&window), 4).is_empty());
	a.content.occurred = Some(window.clone());
	assert_eq!(temporal(&[a.clone(), b], Some(&window), 4), vec![a.id]);
}
#[test]
fn manual_observations_do_not_merge_other_source_categories_into_automatic_groups() {
	let world = unit(1);
	let mut experience = unit(2);
	experience.content.kind = Kind::Experience;
	let mut manual = unit(3);
	manual.content.kind = Kind::Observation;
	manual.content.evidence = vec![world.evidence(), experience.evidence()];
	let mut primary = unit(4);
	primary.content.kind = Kind::Observation;
	primary.content.evidence = vec![
		world.evidence(),
		Evidence::Message {
			id: Uuid::from_u128(55),
			revision: 1,
			digest: "primary".into(),
		},
	];
	let snapshot = vec![world.clone(), experience, manual, primary];
	assert_eq!(
		consolidation::preserve_observations(&[world.clone()], &snapshot, &bounds()).unwrap(),
		vec![world]
	);
}
#[test]
fn derived_memory_cannot_become_verified_or_claim_raw_run_support() {
	let mut content = unit(1).content;
	content.kind = Kind::Observation;
	assert!(content.validate(&bounds()).is_err());
	content.evidence = vec![unit(2).evidence()];
	assert!(content.validate(&bounds()).is_ok());
	content.verification = Verification::Supported;
	assert!(content.validate(&bounds()).is_err());
}
#[test]
fn mutation_requires_client_observed_revision_and_unique_ids() {
	let id = Uuid::from_u128(1);
	assert!(
		Change::Delete {
			id,
			expected_revision: 0
		}
		.validate(&bounds())
		.is_err()
	);
	assert!(
		Change::Delete {
			id,
			expected_revision: 1
		}
		.validate(&bounds())
		.is_ok()
	);
	let entity = Entity {
		name: "  ケント  ".into(),
		category: " Person ".into(),
		aliases: vec![],
	};
	assert_eq!(entity.key(), "person:ケント");
}

#[test]
fn observations_group_conflicts_transitively_and_keep_a_stable_admitted_seed() {
	let mut a = unit(1);
	a.content.text = "Alice lives in Tokyo.".into();
	a.content.entities = vec![Entity {
		name: "Alice".into(),
		category: "person".into(),
		aliases: vec![],
	}];
	let mut b = unit(2);
	b.content.text = "Alice does not live in Tokyo. アリスは東京に住んでいない。".into();
	b.content.entities = vec![
		Entity {
			name: "alice".into(),
			category: "person".into(),
			aliases: vec![],
		},
		Entity {
			name: "東京".into(),
			category: "city".into(),
			aliases: vec![],
		},
	];
	let mut c = unit(3);
	c.content.text = "東京は移動先の候補。".into();
	c.content.entities = vec![Entity {
		name: "東京".into(),
		category: "city".into(),
		aliases: vec![],
	}];
	let snapshot = vec![c.clone(), a.clone(), b.clone()];
	for trigger in [&a, &b, &c] {
		let group = consolidation::sources(trigger, &snapshot, &bounds()).unwrap();
		assert_eq!(
			group.iter().map(|unit| unit.id).collect::<Vec<_>>(),
			vec![a.id, b.id, c.id]
		);
		assert_eq!(
			group[1].content.text, b.content.text,
			"contradictions retain complete original evidence"
		);
	}
	let mut smaller = bounds();
	smaller.max_evidence = 2;
	assert!(
		consolidation::sources(&a, &snapshot, &smaller).is_err(),
		"an oversized connected group cannot silently omit a conflict"
	);
}
#[test]
fn observations_deduplicate_whitespace_without_case_folding_or_crossing_categories() {
	let mut a = unit(1);
	a.content.text = "Use TLS".into();
	let mut b = unit(2);
	b.content.text = "Use   TLS\n".into();
	let mut c = unit(3);
	c.content.text = "Use tls".into();
	let mut experience = unit(4);
	experience.content.kind = Kind::Experience;
	experience.content.text = a.content.text.clone();
	let mut failure = unit(5);
	failure.content.learning = Learning::Failure;
	failure.content.text = a.content.text.clone();
	let mut stale = unit(6);
	stale.stale = true;
	stale.content.text = a.content.text.clone();
	let snapshot = vec![a.clone(), b.clone(), c, experience, failure, stale];
	assert_eq!(
		consolidation::sources(&a, &snapshot, &bounds())
			.unwrap()
			.iter()
			.map(|unit| unit.id)
			.collect::<Vec<_>>(),
		vec![a.id, b.id]
	);
	let mut foreign = unit(7);
	foreign.bank.participant = Some(Uuid::from_u128(999));
	let mut invalid = snapshot;
	invalid.push(foreign);
	assert!(consolidation::sources(&a, &invalid, &bounds()).is_err());
	let mut changed = a.clone();
	changed.revision += 1;
	assert!(consolidation::sources(&changed, &[a], &bounds()).is_err());
}

#[test]
fn graph_resolves_explicit_multilingual_aliases_and_occurrence_edges_with_current_scope() {
	let mut seed = unit(1);
	seed.content.text = "Tokyo is a city.".into();
	seed.content.entities = vec![Entity {
		name: "東京".into(),
		category: "city".into(),
		aliases: vec!["Tokyo".into()],
	}];
	let time = TimeRange {
		start: Utc::now(),
		end: Utc::now() + chrono::Duration::hours(1),
	};
	seed.content.occurred = Some(time.clone());
	let mut alias = unit(2);
	alias.content.text = "東京は都市。".into();
	alias.content.entities = vec![Entity {
		name: " TOKYO ".into(),
		category: "City".into(),
		aliases: vec![],
	}];
	let mut simultaneous = unit(3);
	simultaneous.content.text = "An unrelated event during the same window.".into();
	simultaneous.content.occurred = Some(time);
	let mut causal = unit(4);
	causal.content.text = "The reported consequence.".into();
	seed.content.links = vec![Link {
		target: causal.id,
		revision: causal.revision,
		kind: LinkKind::Causes,
		weight: 0.9,
	}];
	let mut other_bank = alias.clone();
	other_bank.id = Uuid::from_u128(5);
	other_bank.bank.workspace = Uuid::from_u128(999);
	let mut other_kind = alias.clone();
	other_kind.id = Uuid::from_u128(6);
	other_kind.content.entities[0].category = "person".into();
	let mut settings = bounds();
	settings.max_graph_hops = 1;
	let snapshot = vec![
		seed.clone(),
		alias.clone(),
		simultaneous.clone(),
		causal.clone(),
		other_bank,
		other_kind,
	];
	let graph = recall::graph(&snapshot, &[seed.id], &settings);
	assert_eq!(graph, vec![causal.id, simultaneous.id, alias.id]);
	let group = consolidation::sources(&seed, &snapshot[..4], &settings).unwrap();
	assert_eq!(
		group.iter().map(|item| item.id).collect::<Vec<_>>(),
		vec![seed.id, alias.id]
	);
	causal.revision += 1;
	let changed = vec![seed.clone(), alias.clone(), simultaneous.clone(), causal];
	assert_eq!(
		recall::graph(&changed, &[seed.id], &settings),
		vec![simultaneous.id, alias.id]
	);
	let mut stale = alias;
	stale.stale = true;
	assert_eq!(
		recall::graph(&[seed.clone(), stale], &[seed.id], &settings),
		Vec::<Uuid>::new()
	);
}

#[test]
fn aliases_require_distinct_nonempty_bounded_source_names() {
	let mut content = unit(1).content;
	content.entities = vec![Entity {
		name: "Tokyo".into(),
		category: "city".into(),
		aliases: vec!["東京".into()],
	}];
	assert!(content.validate(&bounds()).is_ok());
	content.entities[0].aliases.push(" tokyo ".into());
	assert!(content.validate(&bounds()).is_err());
	content.entities[0].aliases = vec![" ".into()];
	assert!(content.validate(&bounds()).is_err());
}
