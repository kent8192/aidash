use super::*;
use rstest::{fixture, rstest};

#[fixture]
fn account_profile_json() -> Value {
	json!({
		"account_id":"brave-account",
		"agreement_reference":"approved-contract-record",
		"verified_by":"operator",
		"verified_at":"2026-09-01T00:00:00Z",
		"review_after":"2026-10-01T00:00:00Z",
		"storage_permitted":true,
		"query_training_prohibited":true,
		"evaluation_permitted":true,
		"retention_days":90,
		"deletion_after_termination_days":30,
		"credential_env":"AIDASH_SECRET_BRAVE_SEARCH",
		"price_effective_at":"2026-09-01T00:00:00Z",
		"price_review_after":"2026-10-01T00:00:00Z",
		"request_price_micro_usd":5000,
		"monthly_base_fee_micro_usd":0,
		"monthly_limit_micro_usd":20_000_000
	})
}

#[fixture]
fn account_profile(account_profile_json: Value) -> AccountProfile {
	serde_json::from_value(account_profile_json).unwrap()
}

#[fixture]
fn search_time() -> DateTime<Utc> {
	"2026-09-25T00:00:00Z".parse().unwrap()
}

#[fixture]
fn search_request(
	#[default(json!({"query":"example"}))] input: Value,
	#[default(None)] preferred_language: Option<Language>,
) -> ValidatedSearch {
	ValidatedSearch::parse(input, preferred_language).unwrap()
}

#[fixture]
fn provider_results(#[default("https://docs.example.com./a")] url: &str
) -> Vec<u8> {
	serde_json::to_vec(&json!({"web":{"results":[{
		"url":url,"title":"Example","description":"Public discovery hint"
	}]}}))
	.unwrap()
}

#[rstest]
#[case(json!({"query":"example","include_domains":["example.com"]}), 1)]
#[case(json!({"query":"example","exclude_domains":["example.com"]}), 0)]
fn absolute_dns_names_obey_domain_filters(
	#[case] _input: Value,
	#[case] expected_count: usize,
	#[with(_input.clone())] search_request: ValidatedSearch,
	provider_results: Vec<u8>,
	search_time: DateTime<Utc>,
) {
	let output = search_request.normalize_response(&provider_results, search_time);
	assert_eq!(output["data"]["returned_count"], expected_count);
}

#[rstest]
#[case(json!({"query":"example"}), Some(Language::Ja), "ja")]
#[case(json!({"query":"example","language":"en"}), Some(Language::Ja), "en")]
#[case(json!({"query":"example","language":"ja"}), Some(Language::En), "ja")]
#[case(json!({"query":"example"}), None, "en")]
fn search_language_uses_explicit_then_agent_then_english(
	#[case] _input: Value,
	#[case] _preferred_language: Option<Language>,
	#[case] expected_language: &str,
	#[with(_input.clone(), _preferred_language)] search_request: ValidatedSearch,
	provider_results: Vec<u8>,
	search_time: DateTime<Utc>,
) {
	assert_eq!(
		search_request.provider_body()["search_lang"],
		expected_language
	);
	let output = search_request.normalize_response(&provider_results, search_time);
	assert_eq!(output["data"]["effective_language"], expected_language);
}

#[rstest]
fn provider_candidates_have_no_run_source_identity(
	search_request: ValidatedSearch,
	provider_results: Vec<u8>,
	search_time: DateTime<Utc>,
) {
	let output = search_request.normalize_response(&provider_results, search_time);
	assert_eq!(output["data"]["sources"].as_array().unwrap().len(), 1);
	assert!(output["data"]["sources"][0].get("source_id").is_none());
}

struct CredentialEnvironment {
	profile_path: std::path::PathBuf,
	profile: AccountProfile,
}

impl Drop for CredentialEnvironment {
	fn drop(&mut self) {
		let _ = std::fs::remove_file(&self.profile_path);
	}
}

#[fixture]
fn credential_environment(mut account_profile_json: Value) -> CredentialEnvironment {
	let now = Utc::now();
	account_profile_json["verified_at"] = json!(now - chrono::Duration::days(1));
	account_profile_json["review_after"] = json!(now + chrono::Duration::days(1));
	account_profile_json["price_effective_at"] = json!(now - chrono::Duration::days(1));
	account_profile_json["price_review_after"] = json!(now + chrono::Duration::days(1));
	account_profile_json["credential_env"] = json!("AIDASH_SECRET_BRAVE_TEST_EMPTY");
	let profile_path =
		std::env::temp_dir().join(format!("aidash-web-profile-{}.json", uuid::Uuid::new_v4()));
	// Isolate credentials in a child environment, without process-global mutation.
	std::fs::write(
		&profile_path,
		serde_json::to_vec(&account_profile_json).unwrap(),
	)
	.unwrap();
	CredentialEnvironment {
		profile_path,
		profile: serde_json::from_value(account_profile_json).unwrap(),
	}
}

#[rstest]
#[tokio::test]
async fn empty_credential_is_unavailable_before_dispatch(
	credential_environment: CredentialEnvironment,
	search_request: ValidatedSearch,
) {
	if std::env::var_os("AIDASH_WEB_EMPTY_CREDENTIAL_CHILD").is_none() {
		let output = std::process::Command::new(std::env::current_exe().unwrap())
			.args([
				"--exact",
				"web_search::tests::empty_credential_is_unavailable_before_dispatch",
				"--nocapture",
			])
			.env("AIDASH_WEB_EMPTY_CREDENTIAL_CHILD", "1")
			.env(
				"AIDASH_WEB_SEARCH_PROFILE",
				&credential_environment.profile_path,
			)
			.env("AIDASH_SECRET_BRAVE_TEST_EMPTY", "")
			.output()
			.unwrap();
		assert!(
			output.status.success(),
			"{}{}",
			String::from_utf8_lossy(&output.stdout),
			String::from_utf8_lossy(&output.stderr)
		);
		return;
	}
	assert!(AccountProfile::from_env(Utc::now()).is_err());
	assert!(
		credential_environment
			.profile
			.validate_at(Utc::now())
			.is_ok()
	);
	let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
	listener.set_nonblocking(true).unwrap();
	let mut client = BraveClient::new().unwrap();
	client.endpoint =
		Url::parse(&format!("http://{}/search", listener.local_addr().unwrap())).unwrap();
	assert!(
		client
			.search(&search_request, &credential_environment.profile)
			.await
			.is_err()
	);
	assert_eq!(
		listener.accept().unwrap_err().kind(),
		std::io::ErrorKind::WouldBlock
	);
}

#[rstest]
fn account_admission_requires_current_complete_terms(
	mut account_profile: AccountProfile,
	search_time: DateTime<Utc>,
) {
	let now = search_time;
	let profile = &mut account_profile;
	assert!(profile.validate_at(now).is_ok());
	profile.query_training_prohibited = false;
	assert!(profile.validate_at(now).is_err());
	profile.query_training_prohibited = true;
	profile.monthly_base_fee_micro_usd = 20_000_000;
	assert!(profile.validate_at(now).is_err());
	profile.monthly_base_fee_micro_usd = 0;
	profile.price_review_after = "2026-09-24T00:00:00Z".parse().unwrap();
	assert!(profile.validate_at(now).is_err());
	profile.price_review_after = "2026-10-01T00:00:00Z".parse().unwrap();
	profile.credential_env = "INLINE_SECRET".into();
	assert!(profile.validate_at(now).is_err());
	profile.credential_env = "AIDASH_SECRET_BRAVE_SEARCH".into();
	assert!(
		profile
			.validate_at("2026-10-02T00:00:00Z".parse().unwrap())
			.is_err()
	);
}

#[rstest]
#[case(json!({"query":"Rust TaskGroup","language":"en","count":5}), true)]
#[case(json!({"query":"日本語 検索","language":"ja","count":10,"page":9,"freshness":{"from":"2026-09-01","to":"2026-09-25"}}), true)]
#[case(json!({"query":" "}), false)]
#[case(json!({"query":"rust","count":11}), false)]
#[case(json!({"query":"rust","freshness":{"from":"2026-09-26","to":"2026-09-25"}}), false)]
#[case(json!({"query":"rust","include_domains":["example.com"],"exclude_domains":["sub.example.com"]}), false)]
#[case(json!({"query":"rust","country":"jp"}), false)]
#[case(json!({"query":"rust","country":"ZZ"}), false)]
#[case(json!({"query":"rust","unexpected":"value"}), false)]
fn validates_search_contract(#[case] input: Value, #[case] accepted: bool) {
	assert_eq!(ValidatedSearch::parse(input, None).is_ok(), accepted);
}

#[rstest]
fn maps_filters_without_relaxing_them() {
	let input = ValidatedSearch::parse(
		json!({
			"query":"Rust release",
			"language":"ja",
			"include_domains":["rust-lang.org","doc.rust-lang.org"],
			"exclude_domains":["example.net"],
			"freshness":"month"
		}),
		None,
	)
	.unwrap();
	assert_eq!(
		input.outbound_query,
		"Rust release (site:rust-lang.org OR site:doc.rust-lang.org) NOT site:example.net"
	);
	assert_eq!(input.provider_body()["search_lang"], "ja");
	assert_eq!(input.provider_body()["freshness"], "pm");
	assert_eq!(input.provider_body()["spellcheck"], false);
}

#[rstest]
fn public_schema_rejects_unsupported_fields() {
	let schema = jsonschema::validator_for(&search_schema()).unwrap();
	assert!(schema.is_valid(&json!({"query":"日本語","language":"ja"})));
	assert!(!schema.is_valid(&json!({"query":"rust","count":11})));
	assert!(!schema.is_valid(&json!({"query":"rust","provider":"alternate"})));
}

#[rstest]
fn bounds_and_filters_untrusted_provider_metadata() {
	let input = ValidatedSearch::parse(
		json!({
			"query":"example", "include_domains":["example.com"]
		}),
		None,
	)
	.unwrap();
	let raw = json!({"web":{"results":[
		{"url":"https://example.com.attacker.test/","title":"Bad"},
		{"url":"https://docs.example.com/a","title":"長".repeat(300),"description":"文".repeat(700),"page_age":"2 days ago"},
		{"url":"file:///etc/passwd","title":"Bad"}
	]}});
	let output = input.normalize_response(&serde_json::to_vec(&raw).unwrap(), chrono::Utc::now());
	assert_eq!(output["status"], "ok");
	assert_eq!(output["data"]["filtered_count"], 2);
	assert_eq!(output["data"]["sources"].as_array().unwrap().len(), 1);
	assert_eq!(output["data"]["sources"][0]["evidence_state"], "unread");
	assert_eq!(output["data"]["sources"][0]["rank"], 2);
	assert_eq!(output["data"]["sources"][0]["truncated"], true);
	assert!(output.to_string().len() <= MAX_MODEL_BYTES);
}

#[rstest]
fn provider_errors_never_expose_response_body() {
	assert_eq!(
		status_outcome(StatusCode::UNAUTHORIZED)["error"]["code"],
		"provider_auth"
	);
	assert_eq!(
		status_outcome(StatusCode::TOO_MANY_REQUESTS)["error"]["code"],
		"rate_limited"
	);
	assert_eq!(
		status_outcome(StatusCode::SERVICE_UNAVAILABLE)["error"]["code"],
		"provider_unavailable"
	);
	let input = ValidatedSearch::parse(json!({"query":"example"}), None).unwrap();
	assert_eq!(
		input.normalize_response(b"not JSON and not a secret", chrono::Utc::now())["error"]["code"],
		"provider_response_invalid"
	);
}

#[tokio::test]
async fn transport_keeps_credential_out_of_provider_errors() {
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
	let mut client = BraveClient::new().unwrap();
	client.endpoint = Url::parse(&format!("http://{addr}/res/v1/web/search")).unwrap();
	let input = ValidatedSearch::parse(json!({"query":"public fixture"}), None).unwrap();
	let output = client
		.search_with_token(&input, header::HeaderValue::from_static("sentinel-token"))
		.await
		.unwrap();
	server.await.unwrap();
	assert_eq!(output["error"]["code"], "provider_unavailable");
	assert_eq!(output["error"]["retryable"], true);
	assert!(
		!output["error"]["explanation"]
			.as_str()
			.unwrap_or_default()
			.is_empty()
	);
	assert_eq!(output["limits"]["truncated"], false);
	assert!(!output.to_string().contains("sentinel-token"));
}
