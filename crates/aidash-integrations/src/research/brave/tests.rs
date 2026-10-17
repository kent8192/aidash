use super::*;
use rstest::{fixture, rstest};

#[fixture]
fn profile() -> AccountProfile {
	let now = Utc::now();
	AccountProfile {
		account_id: "brave-account".into(),
		agreement_reference: "approved-contract-record".into(),
		verified_by: "deployment_administrator".into(),
		verified_at: now - chrono::Duration::days(1),
		review_after: now + chrono::Duration::days(1),
		storage_permitted: true,
		query_training_prohibited: true,
		evaluation_permitted: true,
		retention_days: 90,
		deletion_after_termination_days: 30,
		terminated_at: None,
		credential_env: "AIDASH_SECRET_BRAVE_SEARCH".into(),
		price_effective_at: now - chrono::Duration::days(1),
		price_review_after: now + chrono::Duration::days(1),
		request_price_micro_usd: 5000,
		monthly_base_fee_micro_usd: 0,
		monthly_limit_micro_usd: 20_000_000,
	}
}

fn query(input: Value) -> SearchQuery {
	SearchQuery::parse(&input, None).unwrap()
}

#[rstest]
fn maps_filters_without_relaxing_them(profile: AccountProfile) {
	let brave = BraveSearch::new(profile, "token").unwrap();
	let query = query(json!({
		"query":"Rust release",
		"language":"ja",
		"include_domains":["rust-lang.org","doc.rust-lang.org"],
		"exclude_domains":["example.net"],
		"freshness":"month"
	}));
	let outbound = brave.disclosure(&query).unwrap();
	assert_eq!(outbound.destination, "api.search.brave.com");
	// The disclosed content is the exact request body sent to Brave.
	let body: Value = serde_json::from_str(&outbound.content).unwrap();
	assert_eq!(
		body["q"],
		"Rust release (site:rust-lang.org OR site:doc.rust-lang.org) NOT site:example.net"
	);
	assert_eq!(body, brave.request(&query).unwrap());
	assert_eq!(body["search_lang"], "ja");
	assert_eq!(body["freshness"], "pm");
	assert_eq!(body["spellcheck"], false);
	assert_eq!(body["result_filter"], json!(["web"]));
	assert_eq!(body["country"], "US");
	let range =
		self::query(json!({"query":"x","freshness":{"from":"2026-09-01","to":"2026-09-25"}}));
	assert_eq!(
		brave.body(&range, "x")["freshness"],
		"2026-09-01to2026-09-25"
	);
}

#[rstest]
fn every_outbound_filter_is_part_of_the_disclosure(profile: AccountProfile) {
	let brave = BraveSearch::new(profile, "token").unwrap();
	let base = json!({"query":"Rust release"});
	let disclosed = |input: Value| brave.disclosure(&query(input)).unwrap().content;
	let original = disclosed(base.clone());
	// An approval for one request cannot cover the same query with other filters.
	for (key, value) in [
		("country", json!("JP")),
		("language", json!("ja")),
		("count", json!(3)),
		("page", json!(2)),
		("freshness", json!("week")),
	] {
		let mut changed = base.clone();
		changed[key] = value;
		assert_ne!(disclosed(changed), original, "{key}");
	}
}

#[rstest]
fn mapped_queries_beyond_provider_limits_are_never_disclosed(profile: AccountProfile) {
	let brave = BraveSearch::new(profile, "token").unwrap();
	let long = query(json!({
		"query": "a ".repeat(70),
		"include_domains": ["a.example","b.example","c.example","d.example","e.example"]
	}));
	assert_eq!(brave.disclosure(&long), Err(Failure::InvalidInput));
}

#[rstest]
fn account_terms_and_credential_are_checked(mut profile: AccountProfile) {
	assert!(BraveSearch::new(profile.clone(), " ").is_err());
	let brave = BraveSearch::new(profile.clone(), "token").unwrap();
	assert_eq!(
		brave.account().unwrap().unwrap().account_id,
		"brave-account"
	);
	profile.query_training_prohibited = false;
	assert!(BraveSearch::new(profile, "token").is_err());
}

#[rstest]
fn parses_only_web_results() {
	let raw = json!({"type":"search","query":{"more_results_available":true},"web":{"results":[
		{"url":"https://example.com/","title":"Example","description":"hint","page_age":"2 days"},
		{"title":"No URL"}
	]}});
	let results = parse_response(&serde_json::to_vec(&raw).unwrap()).unwrap();
	assert_eq!(results.candidates.len(), 1);
	assert!(results.more_results_available);
	assert_eq!(
		parse_response(br#"{"query":{}}"#).unwrap().candidates,
		vec![]
	);
	assert_eq!(
		parse_response(b"not JSON and not a secret"),
		Err(Failure::ProviderResponseInvalid)
	);
	assert_eq!(
		parse_response(br#"{"type":"news"}"#),
		Err(Failure::ProviderResponseInvalid)
	);
}

#[rstest]
#[case(StatusCode::UNAUTHORIZED, Failure::ProviderAuth)]
#[case(StatusCode::TOO_MANY_REQUESTS, Failure::RateLimited)]
#[case(StatusCode::SERVICE_UNAVAILABLE, Failure::ProviderUnavailable)]
#[case(StatusCode::BAD_REQUEST, Failure::ProviderRequestRejected)]
fn provider_statuses_map_to_contract_failures(
	#[case] status: StatusCode,
	#[case] expected: Failure,
) {
	assert_eq!(status_failure(status), expected);
}

#[rstest]
#[tokio::test]
async fn transport_keeps_the_credential_out_of_provider_errors(profile: AccountProfile) {
	use tokio::io::{AsyncReadExt, AsyncWriteExt};
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let addr = listener.local_addr().unwrap();
	let server = tokio::spawn(async move {
		let (mut socket, _) = listener.accept().await.unwrap();
		let mut request = Vec::new();
		let mut buffer = [0u8; 4096];
		loop {
			let read = socket.read(&mut buffer).await.unwrap();
			if read == 0 {
				break;
			}
			request.extend_from_slice(&buffer[..read]);
			if request.windows(4).any(|window| window == b"\r\n\r\n") {
				break;
			}
		}
		let request = String::from_utf8_lossy(&request).to_ascii_lowercase();
		assert!(request.starts_with("post /res/v1/web/search http/1.1"));
		assert!(request.contains("x-subscription-token: sentinel-token"));
		let body = b"sentinel-token: upstream diagnostic";
		let response = format!(
			"HTTP/1.1 503 Service Unavailable\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
			body.len()
		);
		socket.write_all(response.as_bytes()).await.unwrap();
		socket.write_all(body).await.unwrap();
	});
	let brave = BraveSearch::with_endpoint(
		profile,
		"sentinel-token",
		Url::parse(&format!("http://{addr}/res/v1/web/search")).unwrap(),
	)
	.unwrap();
	let outcome = brave
		.search(&query(json!({"query":"public fixture"})))
		.await;
	server.await.unwrap();
	assert_eq!(outcome, Err(Failure::ProviderUnavailable));
	let failure = aidash_domain::research::failure(
		aidash_domain::research::Operation::Search,
		outcome.unwrap_err(),
	);
	assert!(!failure.to_string().contains("sentinel-token"));
	assert_eq!(failure["error"]["retryable"], true);
}

#[rstest]
#[tokio::test]
async fn an_unreachable_endpoint_is_an_uncertain_transport_failure(profile: AccountProfile) {
	let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
	let addr = listener.local_addr().unwrap();
	drop(listener);
	let brave = BraveSearch::with_endpoint(
		profile,
		"token",
		Url::parse(&format!("http://{addr}/res/v1/web/search")).unwrap(),
	)
	.unwrap();
	let outcome = brave
		.search(&query(json!({"query":"x"})))
		.await
		.unwrap_err();
	assert!(outcome.uncertain());
}
