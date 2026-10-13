use super::*;
use rstest::rstest;

fn bounds() -> Bounds {
	serde_json::from_value(serde_json::json!({
		"max_unit_bytes": 1024, "max_input_bytes": 4096, "max_units": 32,
		"max_candidates": 2, "max_entities": 4, "max_evidence": 8, "max_links": 4,
		"max_graph_hops": 2, "max_graph_visits": 8, "max_results": 4,
		"max_context_tokens": 4096, "max_model_calls": 4, "max_model_tokens": 8192,
		"max_cost_micros": 1000, "max_retries": 2, "max_call_seconds": 30
	}))
	.unwrap()
}

fn query(units: Vec<Uuid>) -> ImpactQuery {
	ImpactQuery {
		provider: EntityRef {
			id: "p".into(),
			version: "1.0.0".into(),
		},
		bank: Bank {
			home: "home".into(),
			tenant: "tenant".into(),
			workspace: Uuid::from_u128(9),
			participant: None,
		},
		units,
	}
}

#[rstest]
#[case::current(1, Some(1), false)]
#[case::corrected(1, Some(2), true)]
#[case::deleted_or_withdrawn(1, None, true)]
fn a_read_is_fenced_once_its_revision_is_not_readable(
	#[case] read: i64,
	#[case] readable: Option<i64>,
	#[case] expected: bool,
) {
	assert_eq!(fenced(read, readable), expected);
}

#[rstest]
#[case::empty(vec![], false)]
#[case::one(vec![Uuid::from_u128(1)], true)]
#[case::at_bound(vec![Uuid::from_u128(1), Uuid::from_u128(2)], true)]
#[case::above_bound(vec![Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3)], false)]
#[case::duplicate(vec![Uuid::from_u128(1), Uuid::from_u128(1)], false)]
#[case::nil(vec![Uuid::nil()], false)]
fn the_query_is_bounded_by_the_pinned_candidate_limit(
	#[case] units: Vec<Uuid>,
	#[case] valid: bool,
) {
	assert_eq!(query(units).validate(&bounds()).is_ok(), valid);
}

#[test]
fn the_query_rejects_unknown_fields() {
	let mut value = serde_json::to_value(query(vec![Uuid::from_u128(1)])).unwrap();
	value["include_hidden"] = serde_json::json!(true);
	assert!(serde_json::from_value::<ImpactQuery>(value).is_err());
}
