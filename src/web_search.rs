//! Bounded Brave Web Search adapter. Harness admission, disclosure and durable
//! accounting must authorize a call before this module's transport is wired in.
use crate::{Error, Result, config::secret};
use chrono::{DateTime, NaiveDate, Utc};
use reqwest::{StatusCode, Url, header};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;

// The transport is staged behind the admission work; it is intentionally not
// registered as a callable Harness tool yet.
#[allow(dead_code)]
const BRAVE_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";
const MAX_PROVIDER_BYTES: usize = 1024 * 1024;
const MAX_MODEL_BYTES: usize = 32 * 1024;
// The supported two-letter countries from Brave's Web Search API contract.
const COUNTRIES: &[&str] = &[
	"AR", "AU", "AT", "BE", "BR", "CA", "CL", "DK", "FI", "FR", "DE", "HK", "IN", "ID", "IT", "JP",
	"KR", "MY", "MX", "NL", "NZ", "NO", "CN", "PL", "PT", "PH", "RU", "SA", "ZA", "ES", "SE", "CH",
	"TW", "TR", "GB", "US",
];

/// Operator-attested terms for one paid Brave account. This record is an
/// admission prerequisite, not proof that an agreement has been executed.
/// The actual agreement must be checked and recorded by the operator.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountProfile {
	pub account_id: String,
	pub agreement_reference: String,
	pub verified_by: String,
	pub verified_at: DateTime<Utc>,
	pub review_after: DateTime<Utc>,
	pub storage_permitted: bool,
	pub query_training_prohibited: bool,
	pub evaluation_permitted: bool,
	pub retention_days: u16,
	pub deletion_after_termination_days: u16,
	pub credential_env: String,
	pub price_effective_at: DateTime<Utc>,
	pub price_review_after: DateTime<Utc>,
	pub request_price_micro_usd: u64,
	pub monthly_base_fee_micro_usd: u64,
	pub monthly_limit_micro_usd: u64,
}

impl AccountProfile {
	pub fn validate_at(&self, now: DateTime<Utc>) -> Result<()> {
		if self.account_id.trim().is_empty()
			|| self.agreement_reference.trim().is_empty()
			|| self.verified_by.trim().is_empty()
			|| self.verified_at > now
			|| self.review_after <= now
			|| self.review_after <= self.verified_at
			|| self.price_effective_at > now
			|| self.price_review_after <= now
			|| self.price_review_after <= self.price_effective_at
			|| !self.storage_permitted
			|| !self.query_training_prohibited
			|| !self.evaluation_permitted
			|| self.retention_days == 0
			|| self.deletion_after_termination_days == 0
			|| self.request_price_micro_usd == 0
			|| self.monthly_limit_micro_usd
				< self
					.monthly_base_fee_micro_usd
					.saturating_add(self.request_price_micro_usd)
		{
			return Err(Error::Invalid(
				"web search account terms are unavailable".into(),
			));
		}
		crate::config::validate_secret_reference(&self.credential_env)?;
		Ok(())
	}

	pub fn from_env(now: DateTime<Utc>) -> Result<Option<Self>> {
		let path = match std::env::var("AIDASH_WEB_SEARCH_PROFILE") {
			Ok(path) if !path.is_empty() => path,
			Ok(_) => return Err(Error::Invalid("empty web search profile path".into())),
			Err(std::env::VarError::NotPresent) => return Ok(None),
			Err(_) => return Err(Error::Invalid("invalid web search profile path".into())),
		};
		let profile: Self = serde_json::from_slice(&std::fs::read(path)?)
			.map_err(|_| Error::Invalid("invalid web search profile".into()))?;
		profile.validate_at(now)?;
		secret(&profile.credential_env)?;
		Ok(Some(profile))
	}
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
	Ja,
	En,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DateRange {
	pub from: String,
	pub to: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Freshness {
	Named(String),
	Range(DateRange),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SearchInput {
	pub query: String,
	#[serde(default)]
	pub language: Option<Language>,
	#[serde(default)]
	pub country: Option<String>,
	#[serde(default)]
	pub count: Option<u8>,
	#[serde(default)]
	pub page: Option<u8>,
	#[serde(default)]
	pub freshness: Option<Freshness>,
	#[serde(default)]
	pub include_domains: Vec<String>,
	#[serde(default)]
	pub exclude_domains: Vec<String>,
}

pub fn search_schema() -> Value {
	let domain = json!({"type":"string","minLength":4,"maxLength":253});
	json!({
		"type":"object",
		"required":["query"],
		"properties":{
			"query":{"type":"string","minLength":1,"maxLength":600},
			"language":{"enum":["ja","en"]},
			"country":{"enum":COUNTRIES},
			"count":{"type":"integer","minimum":1,"maximum":10},
			"page":{"type":"integer","minimum":0,"maximum":9},
			"freshness":{"oneOf":[
				{"enum":["day","week","month","year"]},
				{"type":"object","required":["from","to"],"properties":{
					"from":{"type":"string","format":"date"},
					"to":{"type":"string","format":"date"}
				},"additionalProperties":false}
			]},
			"include_domains":{"type":"array","maxItems":5,"items":domain},
			"exclude_domains":{"type":"array","maxItems":5,"items":domain}
		},
		"additionalProperties":false
	})
}

/// A validated request is the only input to the provider adapter. The final
/// outbound query is retained for a later exact-disclosure authorization.
#[derive(Debug, Clone)]
pub struct ValidatedSearch {
	input: SearchInput,
	pub outbound_query: String,
	pub effective_freshness: Option<String>,
}

impl ValidatedSearch {
	pub fn parse(input: Value) -> Result<Self> {
		if serde_json::to_vec(&input)?.len() > 4096 {
			return Err(Error::Invalid("web search input exceeds 4 KiB".into()));
		}
		let mut input: SearchInput = serde_json::from_value(input)
			.map_err(|_| Error::Invalid("invalid web search input".into()))?;
		let query = input.query.trim();
		if query.is_empty() || query.chars().any(char::is_control) {
			return Err(Error::Invalid("invalid web search query".into()));
		}
		input.query = query.to_owned();
		if !valid_query(&input.query) {
			return Err(Error::Invalid(
				"web search query exceeds provider limits".into(),
			));
		}
		if !matches!(input.count.unwrap_or(5), 1..=10) || input.page.unwrap_or(0) > 9 {
			return Err(Error::Invalid("invalid web search count or page".into()));
		}
		if let Some(country) = &input.country
			&& !COUNTRIES.contains(&country.as_str())
		{
			return Err(Error::Invalid("invalid web search country".into()));
		}
		if input.include_domains.len() > 5 || input.exclude_domains.len() > 5 {
			return Err(Error::Invalid("too many web search domains".into()));
		}
		for domain in input
			.include_domains
			.iter()
			.chain(input.exclude_domains.iter())
		{
			if !valid_domain(domain) {
				return Err(Error::Invalid("invalid web search domain".into()));
			}
		}
		for (index, domain) in input.include_domains.iter().enumerate() {
			if input.include_domains[..index].contains(domain)
				|| input.exclude_domains.iter().any(|excluded| {
					domain_matches(domain, excluded) || domain_matches(excluded, domain)
				}) {
				return Err(Error::Invalid("overlapping web search domains".into()));
			}
		}
		for (index, domain) in input.exclude_domains.iter().enumerate() {
			if input.exclude_domains[..index].contains(domain) {
				return Err(Error::Invalid("duplicate web search domain".into()));
			}
		}
		let effective_freshness = input.freshness.as_ref().map(freshness).transpose()?;
		let mut outbound_query = input.query.clone();
		if !input.include_domains.is_empty() {
			outbound_query.push_str(" (");
			outbound_query.push_str(
				&input
					.include_domains
					.iter()
					.map(|host| format!("site:{host}"))
					.collect::<Vec<_>>()
					.join(" OR "),
			);
			outbound_query.push(')');
		}
		for host in &input.exclude_domains {
			outbound_query.push_str(" NOT site:");
			outbound_query.push_str(host);
		}
		if !valid_query(&outbound_query) {
			return Err(Error::Invalid(
				"mapped web search query exceeds provider limits".into(),
			));
		}
		Ok(Self {
			input,
			outbound_query,
			effective_freshness,
		})
	}

	fn provider_body(&self) -> Value {
		let mut body = json!({
			"q": self.outbound_query,
			"count": self.input.count.unwrap_or(5),
			"offset": self.input.page.unwrap_or(0),
			"country": self.input.country.as_deref().unwrap_or("US"),
			"spellcheck": false,
			"text_decorations": false,
			"result_filter": "web",
			"enable_rich_callback": false,
			"operators": true
		});
		// The Brave Web Search schema lists both ja and jp; use the standard
		// ISO 639-1 ja code and verify search quality in the live pilot.
		body["search_lang"] = json!(match self.input.language.unwrap_or(Language::En) {
			Language::Ja => "ja",
			Language::En => "en",
		});
		if let Some(freshness) = &self.effective_freshness {
			body["freshness"] = json!(freshness);
		}
		body
	}

	/// Normalize only Web results. Search snippets are discovery hints, not
	/// evidence that the page itself was read.
	pub fn normalize_response(
		&self,
		bytes: &[u8],
		searched_at: chrono::DateTime<chrono::Utc>,
	) -> Value {
		if bytes.len() > MAX_PROVIDER_BYTES {
			return outcome("error", "response_too_large");
		}
		let Ok(raw) = serde_json::from_slice::<Value>(bytes) else {
			return outcome("error", "provider_response_invalid");
		};
		if raw
			.get("type")
			.and_then(Value::as_str)
			.is_some_and(|kind| kind != "search")
		{
			return outcome("error", "provider_response_invalid");
		}
		let Some(web) = raw.get("web") else {
			if raw.get("query").is_none() {
				return outcome("error", "provider_response_invalid");
			}
			return outcome("empty", "no_results");
		};
		let Some(results) = web.get("results").and_then(Value::as_array) else {
			return outcome("error", "provider_response_invalid");
		};
		let mut sources = Vec::<Value>::new();
		let mut filtered = 0usize;
		let mut truncated = false;
		for (rank, item) in results.iter().enumerate() {
			if sources.len() >= usize::from(self.input.count.unwrap_or(5)) {
				truncated = true;
				break;
			}
			let (Some(raw_url), Some(title)) = (
				item.get("url").and_then(Value::as_str),
				item.get("title").and_then(Value::as_str),
			) else {
				filtered += 1;
				continue;
			};
			let Ok(url) = Url::parse(raw_url) else {
				filtered += 1;
				continue;
			};
			if raw_url.len() > 4096
				|| !matches!(url.scheme(), "http" | "https")
				|| url.host_str().is_none()
				|| !url.username().is_empty()
				|| url.password().is_some()
				|| url.fragment().is_some()
			{
				filtered += 1;
				continue;
			}
			let host = url.host_str().unwrap_or_default();
			if (!self.input.include_domains.is_empty()
				&& !self
					.input
					.include_domains
					.iter()
					.any(|domain| domain_matches(host, domain)))
				|| self
					.input
					.exclude_domains
					.iter()
					.any(|domain| domain_matches(host, domain))
			{
				filtered += 1;
				continue;
			}
			let (title, title_cut) = bounded(title, 512);
			let (snippet, snippet_cut) = bounded(
				item.get("description")
					.and_then(Value::as_str)
					.unwrap_or(""),
				1024,
			);
			sources.push(json!({
				"source_id": uuid::Uuid::new_v4(),
				"url": raw_url,
				"title": title,
				"snippet": snippet,
				"rank": rank + 1,
				"evidence_state": "unread",
				"page_age": item.get("page_age").and_then(Value::as_str).map(|s| bounded(s, 128).0),
				"provider_page_fetched": item.get("page_fetched").and_then(Value::as_str).map(|s| bounded(s, 128).0),
				"truncated": title_cut || snippet_cut
			}));
		}
		let mut result = json!({
			"version": 1,
			"operation": "web_search",
			"status": if sources.is_empty() { "empty" } else { "ok" },
			"provider": "brave",
			"searched_at": searched_at,
			"effective_query": self.outbound_query,
			"effective_language": self.input.language.unwrap_or(Language::En),
			"effective_country": self.input.country.as_deref().unwrap_or("US"),
			"effective_freshness": self.effective_freshness,
			"requested_count": self.input.count.unwrap_or(5),
			"returned_count": sources.len(),
			"filtered_count": filtered,
			"truncated": truncated,
			"more_results_available": raw["query"]["more_results_available"].as_bool().unwrap_or(false),
			"sources": sources
		});
		if result.to_string().len() > MAX_MODEL_BYTES {
			while result.to_string().len() > MAX_MODEL_BYTES {
				let Some(sources) = result["sources"].as_array_mut() else {
					break;
				};
				if sources.pop().is_none() {
					break;
				}
				result["truncated"] = json!(true);
			}
			result["returned_count"] = json!(result["sources"].as_array().map_or(0, Vec::len));
		}
		if result.to_string().len() > MAX_MODEL_BYTES {
			return outcome("error", "response_too_large");
		}
		result
	}
}

fn valid_query(query: &str) -> bool {
	query.chars().count() <= 600 && query.split_whitespace().count() <= 75
}

fn valid_domain(domain: &str) -> bool {
	domain.len() <= 253
		&& domain.contains('.')
		&& domain.split('.').all(|label| {
			!label.is_empty()
				&& label.len() <= 63
				&& !label.starts_with('-')
				&& !label.ends_with('-')
				&& label
					.bytes()
					.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
		})
}

fn domain_matches(host: &str, domain: &str) -> bool {
	host == domain
		|| host
			.strip_suffix(domain)
			.is_some_and(|prefix| prefix.ends_with('.'))
}

fn freshness(value: &Freshness) -> Result<String> {
	match value {
		Freshness::Named(name) => match name.as_str() {
			"day" => Ok("pd".into()),
			"week" => Ok("pw".into()),
			"month" => Ok("pm".into()),
			"year" => Ok("py".into()),
			_ => Err(Error::Invalid("invalid web search freshness".into())),
		},
		Freshness::Range(range) => {
			let from = NaiveDate::parse_from_str(&range.from, "%Y-%m-%d")
				.map_err(|_| Error::Invalid("invalid web search date range".into()))?;
			let to = NaiveDate::parse_from_str(&range.to, "%Y-%m-%d")
				.map_err(|_| Error::Invalid("invalid web search date range".into()))?;
			if from > to || from.to_string() != range.from || to.to_string() != range.to {
				return Err(Error::Invalid("invalid web search date range".into()));
			}
			Ok(format!("{from}to{to}"))
		}
	}
}

fn bounded(value: &str, max: usize) -> (&str, bool) {
	if value.len() <= max {
		return (value, false);
	}
	let mut end = max;
	while !value.is_char_boundary(end) {
		end -= 1;
	}
	(&value[..end], true)
}

fn outcome(status: &str, code: &str) -> Value {
	json!({"version":1,"operation":"web_search","status":status,"error":{"code":code}})
}

fn status_outcome(status: StatusCode) -> Value {
	match status {
		StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => outcome("error", "provider_auth"),
		StatusCode::TOO_MANY_REQUESTS => outcome("error", "rate_limited"),
		StatusCode::REQUEST_TIMEOUT
		| StatusCode::INTERNAL_SERVER_ERROR
		| StatusCode::BAD_GATEWAY
		| StatusCode::SERVICE_UNAVAILABLE
		| StatusCode::GATEWAY_TIMEOUT => outcome("error", "provider_unavailable"),
		_ => outcome("error", "provider_request_rejected"),
	}
}

#[allow(dead_code)]
pub(crate) struct BraveClient {
	client: reqwest::Client,
	endpoint: Url,
}

#[allow(dead_code)]
impl BraveClient {
	pub(crate) fn new() -> Result<Self> {
		let client = reqwest::Client::builder()
			.timeout(Duration::from_secs(15))
			.connect_timeout(Duration::from_secs(5))
			.redirect(reqwest::redirect::Policy::none())
			.no_proxy()
			.user_agent("aidash/0.1 web-search")
			.build()
			.map_err(|_| Error::External("web search client initialization failed".into()))?;
		Ok(Self {
			client,
			endpoint: Url::parse(BRAVE_ENDPOINT).expect("fixed Brave URL is valid"),
		})
	}

	/// Caller must have already reserved a durable attempt and checked the
	/// exact request against current Run authority and disclosure policy.
	pub(crate) async fn search(
		&self,
		input: &ValidatedSearch,
		profile: &AccountProfile,
	) -> Result<Value> {
		profile.validate_at(Utc::now())?;
		let token = secret(&profile.credential_env)?;
		let mut token = header::HeaderValue::from_str(&token)
			.map_err(|_| Error::Invalid("invalid web search credential".into()))?;
		token.set_sensitive(true);
		self.search_with_token(input, token).await
	}

	async fn search_with_token(
		&self,
		input: &ValidatedSearch,
		token: header::HeaderValue,
	) -> Result<Value> {
		let operation = async {
			let searched_at = chrono::Utc::now();
			let mut response = match self
				.client
				.post(self.endpoint.clone())
				.header("x-subscription-token", token)
				.header(header::ACCEPT, "application/json")
				.json(&input.provider_body())
				.send()
				.await
			{
				Ok(response) => response,
				Err(error) if error.is_timeout() => return Ok(outcome("uncertain", "timeout")),
				Err(_) => return Ok(outcome("uncertain", "transport_error")),
			};
			if !response.status().is_success() {
				return Ok(status_outcome(response.status()));
			}
			let mut bytes = Vec::new();
			loop {
				let chunk = match response.chunk().await {
					Ok(chunk) => chunk,
					Err(_) => return Ok(outcome("uncertain", "response_stream_error")),
				};
				let Some(chunk) = chunk else { break };
				if bytes.len().saturating_add(chunk.len()) > MAX_PROVIDER_BYTES {
					return Ok(outcome("error", "response_too_large"));
				}
				bytes.extend_from_slice(&chunk);
			}
			Ok(input.normalize_response(&bytes, searched_at))
		};
		match tokio::time::timeout(Duration::from_secs(15), operation).await {
			Ok(result) => result,
			Err(_) => Ok(outcome("uncertain", "timeout")),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use rstest::rstest;

	fn account_profile() -> AccountProfile {
		serde_json::from_value(json!({
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
		}))
		.unwrap()
	}

	#[rstest]
	fn account_admission_requires_current_complete_terms() {
		let now = "2026-09-25T00:00:00Z".parse().unwrap();
		let mut profile = account_profile();
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
		assert_eq!(ValidatedSearch::parse(input).is_ok(), accepted);
	}

	#[rstest]
	fn maps_filters_without_relaxing_them() {
		let input = ValidatedSearch::parse(json!({
			"query":"Rust release",
			"language":"ja",
			"include_domains":["rust-lang.org","doc.rust-lang.org"],
			"exclude_domains":["example.net"],
			"freshness":"month"
		}))
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
		let input = ValidatedSearch::parse(json!({
			"query":"example", "include_domains":["example.com"]
		}))
		.unwrap();
		let raw = json!({"web":{"results":[
			{"url":"https://example.com.attacker.test/","title":"Bad"},
			{"url":"https://docs.example.com/a","title":"長".repeat(300),"description":"文".repeat(700),"page_age":"2 days ago"},
			{"url":"file:///etc/passwd","title":"Bad"}
		]}});
		let output =
			input.normalize_response(&serde_json::to_vec(&raw).unwrap(), chrono::Utc::now());
		assert_eq!(output["status"], "ok");
		assert_eq!(output["filtered_count"], 2);
		assert_eq!(output["sources"].as_array().unwrap().len(), 1);
		assert_eq!(output["sources"][0]["evidence_state"], "unread");
		assert_eq!(output["sources"][0]["rank"], 2);
		assert_eq!(output["sources"][0]["truncated"], true);
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
		let input = ValidatedSearch::parse(json!({"query":"example"})).unwrap();
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
		let input = ValidatedSearch::parse(json!({"query":"public fixture"})).unwrap();
		let output = client
			.search_with_token(&input, header::HeaderValue::from_static("sentinel-token"))
			.await
			.unwrap();
		server.await.unwrap();
		assert_eq!(output["error"]["code"], "provider_unavailable");
		assert!(!output.to_string().contains("sentinel-token"));
	}
}
