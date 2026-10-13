use super::*;
use crate::memory::recall::{Rankings, fuse, graph, temporal};
use std::collections::{BTreeMap, BTreeSet};

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
fn graph_ranks_all_valid_explicit_and_semantic_neighbors_before_the_link_cap() {
	let mut a = unit(1);
	let weak = unit(2);
	let strong = unit(3);
	let mut settings = bounds();
	settings.max_links = 1;
	settings.max_graph_hops = 1;
	let edge = super::graph::Edge {
		source: a.id,
		source_revision: a.revision,
		target: Link {
			target: strong.id,
			revision: strong.revision,
			kind: LinkKind::Semantic,
			weight: 0.9,
		},
	};
	for revision in [weak.revision, weak.revision + 1] {
		a.content.links = vec![Link {
			target: weak.id,
			revision,
			kind: LinkKind::Causes,
			weight: 0.1,
		}];
		assert_eq!(
			recall::graph_with_edges(
				&[a.clone(), weak.clone(), strong.clone()],
				&[a.id],
				std::slice::from_ref(&edge),
				&settings
			),
			vec![strong.id],
			"weak or stale explicit links must not hide stronger semantic candidates"
		);
	}
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
		consolidation::preserve_observations(std::slice::from_ref(&world), &snapshot, &bounds())
			.unwrap(),
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

#[test]
fn semantic_graph_admission_bounds_vector_work_before_scoring() {
	let mut limits = bounds();
	limits.max_units = 2048;
	assert!(super::graph::validate_capacity(&limits, 3072).is_err());
	assert!(super::graph::validate_capacity(&limits, 3).is_ok());
	limits.max_units = 64;
	assert!(super::graph::validate_capacity(&limits, 3072).is_ok());
	limits.max_units = usize::MAX;
	assert!(super::graph::validate_capacity(&limits, 8192).is_err());
}

fn decay_policy() -> Decay {
	Decay {
		half_life_days: 30,
		prior_floor_millionths: 100_000,
		dormancy: Some(Dormancy {
			threshold_millionths: 100_000,
			interval_hours: 24,
			batch: 16,
			include_preferences: false,
			include_procedures: false,
		}),
	}
}

#[test]
fn retention_score_usage_activation_and_open_bounds() {
	let start: DateTime<Utc> = "2024-01-01T00:00:00Z".parse().unwrap();
	let now = start + chrono::Duration::days(30);
	let decay = decay_policy();
	assert_eq!(
		super::decay::retention_score(&decay, start, None, 0, start, None, now),
		0.5
	);
	assert!(super::decay::retention_score(&decay, start, None, 100, start, None, now) > 0.5);
	assert_eq!(
		super::decay::retention_score(&decay, start, None, 0, now, None, now),
		1.0
	);
	assert_eq!(
		super::decay::retention_score(&decay, start, Some(now), 0, start, None, now),
		1.0
	);
	assert_eq!(
		super::decay::retention_score(&decay, start, None, 0, start, Some(now), now),
		1.0
	);
	assert_eq!(
		super::decay::retention_score(
			&decay,
			start,
			None,
			0,
			start,
			Some(now),
			now + chrono::Duration::days(30)
		),
		0.5
	);
	assert!(
		super::decay::retention_score(
			&decay,
			start,
			None,
			0,
			start,
			None,
			start + chrono::Duration::days(1_000_000)
		) > 0.0
	);
	assert_eq!(super::decay::prior(&decay, 0.5), 0.55);
}

#[test]
fn prior_only_demotes_hits_and_ties_use_retention_then_id() {
	let a = Uuid::from_u128(1);
	let b = Uuid::from_u128(2);
	let missing = Uuid::from_u128(3);
	let allowed = BTreeSet::from([a, b, missing]);
	let ranks = Rankings {
		semantic: vec![a, b],
		keyword: vec![b, a],
		..Default::default()
	};
	let scores = std::collections::BTreeMap::from([(a, 0.1), (b, 1.0), (missing, 1.0)]);
	let baseline = fuse(&ranks, &allowed, 3).unwrap();
	for invalid in [0.0, -1.0, 1.1, f64::NAN, f64::INFINITY] {
		let invalid_scores = std::collections::BTreeMap::from([(a, invalid)]);
		assert!(
			recall::fuse_with_prior(&ranks, &allowed, 3, Some(&decay_policy()), &invalid_scores)
				.is_err()
		);
		assert_eq!(
			recall::fuse_with_prior(&ranks, &allowed, 3, None, &invalid_scores).unwrap(),
			baseline
		);
	}
	assert_eq!(
		recall::fuse_with_prior(&ranks, &allowed, 3, None, &scores).unwrap(),
		baseline
	);
	let ranked =
		recall::fuse_with_prior(&ranks, &allowed, 3, Some(&decay_policy()), &scores).unwrap();
	assert_eq!(ranked.iter().map(|r| r.id).collect::<Vec<_>>(), vec![b, a]);
	assert_eq!(ranked[0].score, baseline[0].score);
	assert!(ranked[1].score < baseline[1].score);
	assert!(recall::compare(a, 0.5, b, 0.5, Some(&scores)).is_gt());
	assert!(recall::compare(a, 0.5, b, 0.5, None).is_lt());
	assert!(
		recall::fuse_with_prior(
			&Rankings {
				semantic: vec![missing],
				..Default::default()
			},
			&BTreeSet::from([a]),
			1,
			Some(&decay_policy()),
			&scores
		)
		.is_err()
	);
}

#[test]
fn pinned_and_live_support_and_learning_exemptions() {
	let mut unit = unit(1);
	let mut decay = decay_policy();
	assert!(super::decay::exempt(&unit, &decay, false, false));
	unit.content.learning = Learning::Procedure;
	assert!(super::decay::exempt(&unit, &decay, false, false));
	let d = decay.dormancy.as_mut().unwrap();
	d.include_preferences = true;
	d.include_procedures = true;
	assert!(!super::decay::exempt(&unit, &decay, false, false));
	assert!(super::decay::exempt(&unit, &decay, true, false));
	assert!(super::decay::exempt(&unit, &decay, false, true));
}

#[derive(Deserialize)]
struct DecayEvaluation {
	decay_cases: Vec<DecayCase>,
}
#[derive(Deserialize)]
struct DecayCase {
	id: String,
	as_of: DateTime<Utc>,
	activated_at: DateTime<Utc>,
	half_life_days: u32,
	prior_floor_millionths: u32,
	semantic: Vec<u128>,
	keyword: Vec<u128>,
	expected_without_decay: Vec<u128>,
	expected_with_decay: Vec<u128>,
	units: Vec<DecayUnit>,
}
#[derive(Deserialize)]
struct DecayUnit {
	id: u128,
	learned_at: DateTime<Utc>,
	last_delivered_at: Option<DateTime<Utc>>,
	deliveries: u64,
	pinned: bool,
}
#[test]
fn fixed_clock_decay_evaluation() {
	let fixture: DecayEvaluation = toml::from_str(include_str!(
		"../../../../tests/fixtures/native_memory_evaluation.toml"
	))
	.unwrap();
	for case in fixture.decay_cases {
		let decay = Decay {
			half_life_days: case.half_life_days,
			prior_floor_millionths: case.prior_floor_millionths,
			dormancy: None,
		};
		let scores = case
			.units
			.iter()
			.map(|u| {
				(
					Uuid::from_u128(u.id),
					if u.pinned {
						1.0
					} else {
						super::decay::retention_score(
							&decay,
							u.learned_at,
							u.last_delivered_at,
							u.deliveries,
							case.activated_at,
							None,
							case.as_of,
						)
					},
				)
			})
			.collect();
		let allowed = case.units.iter().map(|u| Uuid::from_u128(u.id)).collect();
		let ranks = Rankings {
			semantic: case.semantic.into_iter().map(Uuid::from_u128).collect(),
			keyword: case.keyword.into_iter().map(Uuid::from_u128).collect(),
			..Default::default()
		};
		for (variant, expected) in [
			(None, case.expected_without_decay),
			(Some(&decay), case.expected_with_decay),
		] {
			let actual = recall::fuse_with_prior(&ranks, &allowed, 2, variant, &scores).unwrap();
			assert_eq!(
				actual.iter().map(|r| r.id.as_u128()).collect::<Vec<_>>(),
				expected,
				"{}",
				case.id
			);
		}
	}
}

#[test]
fn decay_policy_validation_and_legacy_serde_byte_identity() {
	let baseline =
		include_str!("../../../../tests/fixtures/native_memory_policy_baseline.json").trim();
	let mut policy: Policy = serde_json::from_str(baseline).unwrap();
	assert!(policy.decay.is_none());
	policy.validate().unwrap();
	assert_eq!(serde_json::to_string(&policy).unwrap(), baseline);
	let valid = decay_policy();
	policy.decay = Some(valid.clone());
	policy.validate().unwrap();
	for invalid in [
		Decay {
			half_life_days: 0,
			..valid.clone()
		},
		Decay {
			half_life_days: 3651,
			..valid.clone()
		},
		Decay {
			prior_floor_millionths: 1_000_001,
			..valid.clone()
		},
	] {
		policy.decay = Some(invalid);
		assert!(policy.validate().is_err());
	}
	for (threshold, interval, batch) in [
		(0, 24, 16),
		(1_000_000, 24, 16),
		(1, 0, 16),
		(1, 8761, 16),
		(1, 1, 0),
		(1, 1, 1025),
	] {
		let mut d = valid.clone();
		let dormancy = d.dormancy.as_mut().unwrap();
		dormancy.threshold_millionths = threshold;
		dormancy.interval_hours = interval;
		dormancy.batch = batch;
		policy.decay = Some(d);
		assert!(policy.validate().is_err());
	}
	for (half_life, floor, threshold, interval, batch) in
		[(1, 0, 1, 1, 1), (3650, 1_000_000, 999_999, 8760, 1024)]
	{
		let mut d = valid.clone();
		d.half_life_days = half_life;
		d.prior_floor_millionths = floor;
		let dormancy = d.dormancy.as_mut().unwrap();
		dormancy.threshold_millionths = threshold;
		dormancy.interval_hours = interval;
		dormancy.batch = batch;
		policy.decay = Some(d);
		policy.validate().unwrap();
	}
	assert!(serde_json::from_value::<Policy>(serde_json::json!({"dormancy": {}})).is_err());
	policy.decay = None;
	assert_eq!(serde_json::to_string(&policy).unwrap(), baseline);
}

fn hit(id: u128, score: f64) -> recall::Ranked {
	recall::Ranked {
		id: Uuid::from_u128(id),
		score,
	}
}
fn vectors(entries: &[(u128, [f32; 2])]) -> BTreeMap<Uuid, Vec<f32>> {
	entries
		.iter()
		.map(|(id, values)| (Uuid::from_u128(*id), values.to_vec()))
		.collect()
}
fn order(result: &recall::Diversified) -> Vec<u128> {
	result.selected.iter().map(|hit| hit.id.as_u128()).collect()
}
const LAMBDA: u32 = 700_000;

#[test]
fn diversification_prefers_a_distinct_unit_over_paraphrases() {
	// 1..=3 are paraphrases; 4 is distinct and slightly less relevant.
	let ranked = [hit(1, 1.0), hit(2, 0.95), hit(3, 0.9), hit(4, 0.8)];
	let vectors = vectors(&[
		(1, [1., 0.]),
		(2, [0.99, 0.01]),
		(3, [0.98, 0.02]),
		(4, [0., 1.]),
	]);
	let result =
		recall::diversify(&ranked, &vectors, &recall::Related::new(), None, LAMBDA, 4).unwrap();
	assert_eq!(order(&result), [1, 4, 2, 3]);
	assert!(result.skipped.is_empty());
	let two =
		recall::diversify(&ranked, &vectors, &recall::Related::new(), None, LAMBDA, 2).unwrap();
	assert_eq!(order(&two), [1, 4]);
	// λ = 1 is pure relevance: the baseline order survives.
	let relevance = recall::diversify(
		&ranked,
		&vectors,
		&recall::Related::new(),
		None,
		1_000_000,
		4,
	)
	.unwrap();
	assert_eq!(order(&relevance), [1, 2, 3, 4]);
}

#[test]
fn diversification_is_deterministic_and_stays_inside_the_candidates() {
	let vectors = vectors(&[(1, [1., 0.]), (2, [1., 0.]), (3, [0., 1.]), (9, [1., 1.])]);
	let forward = [hit(1, 0.5), hit(2, 0.5), hit(3, 0.5)];
	let backward = [hit(3, 0.5), hit(2, 0.5), hit(1, 0.5)];
	let first =
		recall::diversify(&forward, &vectors, &recall::Related::new(), None, LAMBDA, 3).unwrap();
	let second = recall::diversify(
		&backward,
		&vectors,
		&recall::Related::new(),
		None,
		LAMBDA,
		3,
	)
	.unwrap();
	assert_eq!(first, second);
	assert_eq!(order(&first), [1, 3, 2], "ties use the lowest Unit ID");
	// A vector for an ID outside the candidates (9) never adds a result.
	assert!(
		first
			.selected
			.iter()
			.all(|hit| hit.id != Uuid::from_u128(9))
	);
	let candidates: BTreeSet<_> = forward.iter().map(|hit| hit.id).collect();
	assert!(
		first
			.selected
			.iter()
			.all(|hit| candidates.contains(&hit.id))
	);
	assert!(
		recall::diversify(&forward, &vectors, &recall::Related::new(), None, LAMBDA, 0)
			.unwrap()
			.selected
			.is_empty()
	);
}

#[test]
fn a_candidate_without_a_current_vector_is_kept_by_relevance_and_recorded() {
	let ranked = [hit(1, 1.0), hit(2, 0.9), hit(3, 0.8)];
	// 2 has no vector: it is neither dropped nor penalized, only reported.
	let vectors = vectors(&[(1, [1., 0.]), (3, [1., 0.])]);
	let result =
		recall::diversify(&ranked, &vectors, &recall::Related::new(), None, LAMBDA, 3).unwrap();
	assert_eq!(order(&result), [1, 2, 3]);
	assert_eq!(result.skipped, [Uuid::from_u128(2)]);
	let none = recall::diversify(
		&ranked,
		&BTreeMap::new(),
		&recall::Related::new(),
		None,
		LAMBDA,
		3,
	)
	.unwrap();
	assert_eq!(order(&none), [1, 2, 3]);
	assert_eq!(none.skipped.len(), 3);
}

#[test]
fn an_observation_and_its_sources_count_as_maximally_similar_so_lambda_still_decides() {
	let mut observation = unit(10);
	observation.content.kind = Kind::Observation;
	observation.content.evidence = [1, 2]
		.map(|id| Evidence::Unit {
			bank: observation.bank.clone(),
			id: Uuid::from_u128(id),
			revision: 1,
		})
		.to_vec();
	let sources = [unit(1), unit(2)];
	let unrelated = unit(3);
	let mut stale_citation = unit(4);
	stale_citation.revision = 2;
	let mut stale_observation = unit(11);
	stale_observation.content.kind = Kind::Observation;
	stale_observation.content.evidence = vec![Evidence::Unit {
		bank: stale_observation.bank.clone(),
		id: Uuid::from_u128(4),
		revision: 1,
	}];
	let units = [
		observation,
		sources[0].clone(),
		sources[1].clone(),
		unrelated,
		stale_citation,
		stale_observation,
	];
	let related = recall::related(&units);
	assert_eq!(
		related[&Uuid::from_u128(10)],
		BTreeSet::from([Uuid::from_u128(1), Uuid::from_u128(2)])
	);
	assert!(related[&Uuid::from_u128(1)].contains(&Uuid::from_u128(10)));
	assert!(
		!related.contains_key(&Uuid::from_u128(11)) && !related.contains_key(&Uuid::from_u128(4)),
		"a citation of another revision is not redundancy"
	);
	let vectors = vectors(&[
		(10, [1., 0.]),
		(1, [0.6, 0.8]),
		(2, [0.8, 0.6]),
		(3, [0., 1.]),
	]);
	// A reasonably relevant unrelated Unit takes the second slot from the summary's sources.
	let ranked = [hit(10, 1.0), hit(1, 0.9), hit(2, 0.8), hit(3, 0.5)];
	let two = recall::diversify(&ranked, &vectors, &related, None, LAMBDA, 2).unwrap();
	assert_eq!(order(&two), [10, 3], "the unrelated Unit is not excluded");
	let all = recall::diversify(&ranked, &vectors, &related, None, LAMBDA, 4).unwrap();
	assert_eq!(
		order(&all),
		[10, 3, 1, 2],
		"sources are still delivered when the budget allows"
	);
	// A near-zero-relevance Unit cannot push out a highly relevant source: λ still governs.
	let weak = [hit(10, 1.0), hit(1, 0.9), hit(2, 0.8), hit(3, 0.05)];
	let two = recall::diversify(&weak, &vectors, &related, None, LAMBDA, 2).unwrap();
	assert_eq!(order(&two), [10, 1]);
}

#[test]
fn exact_ties_follow_retention_before_unit_id() {
	let ranked = [hit(1, 0.5), hit(2, 0.5)];
	let vectors = vectors(&[(1, [1., 0.]), (2, [0., 1.])]);
	let related = recall::Related::new();
	let plain = recall::diversify(&ranked, &vectors, &related, None, LAMBDA, 2).unwrap();
	assert_eq!(order(&plain), [1, 2]);
	let retention = BTreeMap::from([(Uuid::from_u128(1), 0.2), (Uuid::from_u128(2), 0.9)]);
	let kept = recall::diversify(&ranked, &vectors, &related, Some(&retention), LAMBDA, 2).unwrap();
	assert_eq!(order(&kept), [2, 1]);
}

#[test]
fn diversification_rejects_invalid_inputs() {
	let ranked = [hit(1, 1.0), hit(2, 0.5)];
	let related = recall::Related::new();
	let good = vectors(&[(1, [1., 0.]), (2, [0., 1.])]);
	for lambda in [0, 1_000_001] {
		assert!(recall::diversify(&ranked, &good, &related, None, lambda, 2).is_err());
	}
	assert!(
		recall::diversify(
			&[hit(1, 1.0), hit(1, 0.5)],
			&good,
			&related,
			None,
			LAMBDA,
			2
		)
		.is_err()
	);
	assert!(recall::diversify(&[hit(1, f64::NAN)], &good, &related, None, LAMBDA, 1).is_err());
	for bad in [[0., 0.], [f32::NAN, 1.], [f32::INFINITY, 1.]] {
		let vectors = vectors(&[(1, [1., 0.]), (2, bad)]);
		assert!(recall::diversify(&ranked, &vectors, &related, None, LAMBDA, 2).is_err());
	}
	let mut mixed = good.clone();
	mixed.insert(Uuid::from_u128(2), vec![1.]);
	assert!(recall::diversify(&ranked, &mixed, &related, None, LAMBDA, 2).is_err());
	// Extreme finite scores cannot overflow the relevance span.
	let extreme = [hit(1, f64::MAX), hit(2, f64::MIN)];
	let result = recall::diversify(&extreme, &good, &related, None, LAMBDA, 2).unwrap();
	assert_eq!(order(&result), [1, 2]);
}

#[test]
fn diversity_lambda_is_optional_validated_and_omitted_when_unset() {
	let baseline =
		include_str!("../../../../tests/fixtures/native_memory_policy_baseline.json").trim();
	let mut policy: Policy = serde_json::from_str(baseline).unwrap();
	assert_eq!(policy.diversity_lambda_millionths, None);
	assert_eq!(serde_json::to_string(&policy).unwrap(), baseline);
	for lambda in [1, 700_000, 1_000_000] {
		policy.diversity_lambda_millionths = Some(lambda);
		policy.validate().unwrap();
		let encoded = serde_json::to_string(&policy).unwrap();
		assert!(encoded.contains(&format!("\"diversity_lambda_millionths\":{lambda}")));
		assert_eq!(serde_json::from_str::<Policy>(&encoded).unwrap(), policy);
	}
	for lambda in [0, 1_000_001] {
		policy.diversity_lambda_millionths = Some(lambda);
		assert!(policy.validate().is_err());
	}
	policy.diversity_lambda_millionths = None;
	assert_eq!(serde_json::to_string(&policy).unwrap(), baseline);
}
