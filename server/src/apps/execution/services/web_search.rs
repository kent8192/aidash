//! Bounded Brave Web Search adapter. Harness admission, disclosure and durable
//! accounting must authorize a call before this module's transport is wired in.
use crate::{Error, Result, config::secret};
use chrono::{DateTime, NaiveDate, Utc};
use reqwest::{StatusCode, Url, header};
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
		search_credential(&profile.credential_env)?;
		Ok(Some(profile))
	}
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
	effective_language: Language,
	pub outbound_query: String,
	pub effective_freshness: Option<String>,
}

impl ValidatedSearch {
	/// The caller supplies the Agent's supported preferred language. An explicit
	/// argument takes precedence; no supported preference falls back to English.
	pub fn parse(input: Value, preferred_language: Option<Language>) -> Result<Self> {
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
			effective_language: input
				.language
				.or(preferred_language)
				.unwrap_or(Language::En),
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
		body["search_lang"] = json!(match self.effective_language {
			Language::Ja => "ja",
			Language::En => "en",
		});
		if let Some(freshness) = &self.effective_freshness {
			body["freshness"] = json!(freshness);
		}
		body
	}

	/// Normalize only Web results. Search snippets are discovery hints, not
	/// evidence that the page itself was read. These are provider candidates:
	/// the Harness must persist them under a Run and assign immutable source IDs
	/// before exposing them as Agent-facing source records.
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
		let results: &[Value] = match raw.get("web") {
			Some(web) => match web.get("results").and_then(Value::as_array) {
				Some(results) => results,
				None => return outcome("error", "provider_response_invalid"),
			},
			None if raw.get("query").is_some() => &[],
			None => return outcome("error", "provider_response_invalid"),
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
				"url": raw_url,
				"title": title,
				"snippet": snippet,
				"rank": rank + 1,
				"evidence_state": "unread",
				"page_age": item.get("page_age").and_then(Value::as_str).map(|s| bounded(s, 128).0),
				"provider_page_fetched": item.get("page_fetched").and_then(Value::as_str).map(|s| bounded(s, 128).0),
				"truncated": title_cut || snippet_cut
			}));
			truncated |= title_cut || snippet_cut;
		}
		let mut result = json!({
			"version": 1,
			"operation": "web_search",
			"status": if sources.is_empty() { "empty" } else { "ok" },
			"limits": {"max_bytes": MAX_MODEL_BYTES, "truncated": truncated, "continuation": null},
			"data": {
			"provider": "brave",
			"searched_at": searched_at,
			"effective_query": self.outbound_query,
			"effective_language": self.effective_language,
			"effective_country": self.input.country.as_deref().unwrap_or("US"),
			"effective_freshness": self.effective_freshness,
			"requested_count": self.input.count.unwrap_or(5),
			"returned_count": sources.len(),
			"filtered_count": filtered,
			"truncated": truncated,
			"more_results_available": raw["query"]["more_results_available"].as_bool().unwrap_or(false),
			"sources": sources
			}
		});
		if result.to_string().len() > MAX_MODEL_BYTES {
			while result.to_string().len() > MAX_MODEL_BYTES {
				let Some(sources) = result["data"]["sources"].as_array_mut() else {
					break;
				};
				if sources.pop().is_none() {
					break;
				}
				result["data"]["truncated"] = json!(true);
				result["limits"]["truncated"] = json!(true);
			}
			let returned_count = result["data"]["sources"].as_array().map_or(0, Vec::len);
			result["data"]["returned_count"] = json!(returned_count);
			if returned_count == 0 {
				result["status"] = json!("empty");
			}
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
	// DNS absolute names retain their root dot in URL hosts. It must not change
	// the domain restriction applied to the equivalent relative DNS name.
	let host = host.trim_end_matches('.');
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
	let (explanation, retryable) = match code {
		"provider_auth" => (
			"The search provider rejected the configured credential.",
			false,
		),
		"rate_limited" => ("The search provider rate limit was reached.", true),
		"provider_unavailable" => ("The search provider is temporarily unavailable.", true),
		"provider_request_rejected" => ("The search provider rejected the request.", false),
		"provider_response_invalid" => ("The search provider returned an invalid response.", false),
		"response_too_large" => (
			"The search response exceeded the configured size limit.",
			false,
		),
		"timeout" => (
			"The search deadline expired; the provider outcome is unknown.",
			false,
		),
		"transport_error" => (
			"The search transport failed; the provider outcome is unknown.",
			false,
		),
		"response_stream_error" => (
			"The search response was interrupted; the provider outcome is unknown.",
			false,
		),
		_ => ("The search operation failed.", false),
	};
	json!({
		"version":1,"operation":"web_search","status":status,
		"error":{"code":code,"explanation":explanation,"retryable":retryable},
		"limits":{"max_bytes":MAX_MODEL_BYTES,"truncated":false,"continuation":null}
	})
}

fn search_credential(name: &str) -> Result<header::HeaderValue> {
	let token = secret(name)?;
	if token.trim().is_empty() {
		return Err(Error::Invalid(
			"web search credential is not configured".into(),
		));
	}
	let mut token = header::HeaderValue::from_str(&token)
		.map_err(|_| Error::Invalid("invalid web search credential".into()))?;
	token.set_sensitive(true);
	Ok(token)
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
		let token = search_credential(&profile.credential_env)?;
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
#[path = "../tests/services_web_search_tests.rs"]
mod tests;

pub use crate::apps::execution::serializers::web_search::{
	AccountProfile, DateRange, Freshness, Language, SearchInput,
};
