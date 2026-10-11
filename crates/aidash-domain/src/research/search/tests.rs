use super::*;
use rstest::{fixture, rstest};

#[fixture]
fn candidate(#[default("https://docs.example.com./a")] url: &str
) -> ProviderResults {
	ProviderResults {
		candidates: vec![ProviderCandidate {
			url: url.into(),
			title: "Example".into(),
			snippet: Some("Public discovery hint".into()),
			page_age: None,
		}],
		more_results_available: false,
	}
}

#[rstest]
#[case(json!({"query":"Rust TaskGroup","language":"en","count":5}), true)]
#[case(json!({"query":"日本語 検索","language":"ja","count":10,"page":9,"freshness":{"from":"2026-09-01","to":"2026-09-25"}}), true)]
#[case(json!({"query":"rust","freshness":"month"}), true)]
#[case(json!({"query":" "}), false)]
#[case(json!({"query":"rust","count":11}), false)]
#[case(json!({"query":"rust","count":0}), false)]
#[case(json!({"query":"rust","freshness":{"from":"2026-09-26","to":"2026-09-25"}}), false)]
#[case(json!({"query":"rust","freshness":{"from":"2026-9-1","to":"2026-09-25"}}), false)]
#[case(json!({"query":"rust","freshness":"hour"}), false)]
#[case(json!({"query":"rust","include_domains":["example.com"],"exclude_domains":["sub.example.com"]}), false)]
#[case(json!({"query":"rust","include_domains":["Example.com"]}), false)]
#[case(json!({"query":"rust","country":"jp"}), false)]
#[case(json!({"query":"rust","country":"ZZ"}), false)]
#[case(json!({"query":"rust","unexpected":"value"}), false)]
#[case(json!({"query":"a\u{0007}b"}), false)]
fn validates_the_search_contract(#[case] input: Value, #[case] accepted: bool) {
	assert_eq!(
		SearchQuery::parse(&input, None).is_ok(),
		accepted,
		"{input}"
	);
}

#[rstest]
#[case(json!({"query":"example"}), Some(Language::Ja), Language::Ja)]
#[case(json!({"query":"example","language":"en"}), Some(Language::Ja), Language::En)]
#[case(json!({"query":"example","language":"ja"}), Some(Language::En), Language::Ja)]
#[case(json!({"query":"example"}), None, Language::En)]
fn language_uses_explicit_then_agent_then_english(
	#[case] input: Value,
	#[case] preferred: Option<Language>,
	#[case] expected: Language,
) {
	assert_eq!(
		SearchQuery::parse(&input, preferred).unwrap().language,
		expected
	);
}

#[rstest]
#[case(json!({"query":"example","include_domains":["example.com"]}), 1)]
#[case(json!({"query":"example","exclude_domains":["example.com"]}), 0)]
fn absolute_dns_names_obey_domain_filters(
	#[case] input: Value,
	#[case] expected: usize,
	candidate: ProviderResults,
) {
	let page = SearchQuery::parse(&input, None)
		.unwrap()
		.normalize(candidate);
	assert_eq!(page.sources.len(), expected);
	assert_eq!(page.filtered_count, 1 - expected);
}

#[rstest]
fn host_filters_and_bounds_untrusted_provider_candidates() {
	let query = SearchQuery::parse(
		&json!({"query":"example","include_domains":["example.com"]}),
		None,
	)
	.unwrap();
	let candidate = |url: &str, title: String, snippet: Option<String>| ProviderCandidate {
		url: url.into(),
		title,
		snippet,
		page_age: Some("2 days ago".into()),
	};
	let page = query.normalize(ProviderResults {
		candidates: vec![
			candidate("https://example.com.attacker.test/", "Bad".into(), None),
			candidate(
				"https://docs.example.com/a",
				"長".repeat(300),
				Some("文".repeat(700)),
			),
			candidate("file:///etc/passwd", "Bad".into(), None),
			candidate("https://user:secret@example.com/", "Bad".into(), None),
			candidate("https://example.com/#fragment", "Bad".into(), None),
		],
		more_results_available: true,
	});
	assert_eq!(page.filtered_count, 4);
	assert_eq!(page.sources.len(), 1);
	let source = &page.sources[0];
	assert_eq!(source.rank, 2);
	assert!(source.truncated && page.truncated);
	assert!(source.title.len() <= 512 && source.snippet.len() <= 1024);
	assert!(page.more_results_available);
}

#[rstest]
fn normalization_keeps_at_most_the_requested_count() {
	let query = SearchQuery::parse(&json!({"query":"example","count":1}), None).unwrap();
	let candidates = (0..3)
		.map(|index| ProviderCandidate {
			url: format!("https://example.com/{index}"),
			title: "Example".into(),
			snippet: None,
			page_age: None,
		})
		.collect();
	let page = query.normalize(ProviderResults {
		candidates,
		more_results_available: false,
	});
	assert_eq!(page.sources.len(), 1);
	assert!(page.truncated);
}

#[rstest]
fn public_schema_rejects_unsupported_fields() {
	let schema = jsonschema::validator_for(&search_schema()).unwrap();
	assert!(schema.is_valid(&json!({"query":"日本語","language":"ja"})));
	assert!(!schema.is_valid(&json!({"query":"rust","count":11})));
	assert!(!schema.is_valid(&json!({"query":"rust","provider":"alternate"})));
}

#[rstest]
fn effective_search_reports_defaults() {
	let query = SearchQuery::parse(&json!({"query":" spaced ","freshness":"week"}), None).unwrap();
	let effective = query.effective();
	assert_eq!(effective["query"], "spaced");
	assert_eq!(effective["country"], "US");
	assert_eq!(effective["count"], 5);
	assert_eq!(effective["freshness"], "week");
}
