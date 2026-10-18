//! Brave Web Search as a research search provider. Its key is a Deployment
//! secret named by the provider's settings, never a Provider Credential. The
//! host attributes every request to the Research account holder's account.
use crate::research::Failure;
use aidash_application::ports::research::{Outbound, SearchProvider};
use aidash_domain::research::{
	Operation,
	account::AccountTerms,
	search::{Language, Period, ProviderCandidate, ProviderResults, SearchQuery, ValidFreshness},
};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::{StatusCode, Url, header};
use serde_json::{Value, json};
use std::time::Duration;

const ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";
const MAX_PROVIDER_BYTES: usize = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(15);

pub struct BraveSearch {
	client: reqwest::Client,
	endpoint: Url,
	terms: AccountTerms,
	token: header::HeaderValue,
}

impl BraveSearch {
	/// `token` is the value of the Deployment secret the settings name. Terms
	/// that are incomplete are refused; terms that lapse only leave the
	/// provider without a current account.
	pub fn new(terms: AccountTerms, token: &str) -> crate::Result<Self> {
		terms.validate(&[Operation::Search])?;
		Self::with_endpoint(terms, token, Url::parse(ENDPOINT).expect("fixed Brave URL"))
	}

	/// A Brave-compatible endpoint other than the public API, for fixtures.
	pub fn with_endpoint(terms: AccountTerms, token: &str, endpoint: Url) -> crate::Result<Self> {
		if token.trim().is_empty() {
			return Err(crate::Error::Invalid(
				"search credential is not configured".into(),
			));
		}
		let mut token = header::HeaderValue::from_str(token)
			.map_err(|_| crate::Error::Invalid("invalid search credential".into()))?;
		token.set_sensitive(true);
		let client = reqwest::Client::builder()
			.timeout(TIMEOUT)
			.connect_timeout(Duration::from_secs(5))
			.redirect(reqwest::redirect::Policy::none())
			.no_proxy()
			.user_agent(concat!(
				"aidash/",
				env!("CARGO_PKG_VERSION"),
				" research-search"
			))
			.build()
			.map_err(crate::http_error)?;
		Ok(Self {
			client,
			endpoint,
			terms,
			token,
		})
	}

	/// The complete request body sent to Brave. It is also the disclosed
	/// content, so an approval covers every outbound value, not only the query.
	fn request(&self, query: &SearchQuery) -> Result<Value, Failure> {
		let outbound = outbound_query(query);
		if !aidash_domain::research::search::valid_query(&outbound) {
			return Err(Failure::InvalidInput);
		}
		Ok(self.body(query, &outbound))
	}

	fn body(&self, query: &SearchQuery, outbound: &str) -> Value {
		let mut body = json!({
			"q": outbound,
			"count": query.count,
			"offset": query.page,
			"country": query.country,
			"spellcheck": false,
			"text_decorations": false,
			// The POST schema types the filter as `string[]`.
			"result_filter": ["web"],
			"enable_rich_callback": false,
			"operators": true,
			// The Brave schema lists both ja and jp; use the ISO 639-1 code.
			"search_lang": match query.language {
				Language::Ja => "ja",
				Language::En => "en",
			},
		});
		if let Some(freshness) = query.freshness {
			body["freshness"] = json!(match freshness {
				ValidFreshness::Period(Period::Day) => "pd".to_owned(),
				ValidFreshness::Period(Period::Week) => "pw".to_owned(),
				ValidFreshness::Period(Period::Month) => "pm".to_owned(),
				ValidFreshness::Period(Period::Year) => "py".to_owned(),
				ValidFreshness::Range { from, to } => format!("{from}to{to}"),
			});
		}
		body
	}
}

/// Domain filters become search operators; the host filters results again.
pub fn outbound_query(query: &SearchQuery) -> String {
	let mut outbound = query.query.clone();
	if !query.include_domains.is_empty() {
		outbound.push_str(" (");
		outbound.push_str(
			&query
				.include_domains
				.iter()
				.map(|host| format!("site:{host}"))
				.collect::<Vec<_>>()
				.join(" OR "),
		);
		outbound.push(')');
	}
	for host in &query.exclude_domains {
		outbound.push_str(" NOT site:");
		outbound.push_str(host);
	}
	outbound
}

/// Parse only Web results. Snippets are discovery hints, never evidence.
pub fn parse_response(bytes: &[u8]) -> Result<ProviderResults, Failure> {
	if bytes.len() > MAX_PROVIDER_BYTES {
		return Err(Failure::ResponseTooLarge);
	}
	let raw: Value = serde_json::from_slice(bytes).map_err(|_| Failure::ProviderResponseInvalid)?;
	if raw
		.get("type")
		.and_then(Value::as_str)
		.is_some_and(|kind| kind != "search")
	{
		return Err(Failure::ProviderResponseInvalid);
	}
	let results: &[Value] = match raw.get("web") {
		Some(web) => web
			.get("results")
			.and_then(Value::as_array)
			.ok_or(Failure::ProviderResponseInvalid)?,
		None if raw.get("query").is_some() => &[],
		None => return Err(Failure::ProviderResponseInvalid),
	};
	let text = |item: &Value, key: &str| item.get(key).and_then(Value::as_str).map(str::to_owned);
	Ok(ProviderResults {
		candidates: results
			.iter()
			.filter_map(|item| {
				Some(ProviderCandidate {
					url: text(item, "url")?,
					title: text(item, "title")?,
					snippet: text(item, "description"),
					page_age: text(item, "page_age"),
				})
			})
			.collect(),
		more_results_available: raw["query"]["more_results_available"]
			.as_bool()
			.unwrap_or(false),
	})
}

/// Provider statuses map to contract failures; bodies are never exposed.
pub fn status_failure(status: StatusCode) -> Failure {
	match status {
		StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Failure::ProviderAuth,
		StatusCode::TOO_MANY_REQUESTS => Failure::RateLimited,
		StatusCode::REQUEST_TIMEOUT
		| StatusCode::INTERNAL_SERVER_ERROR
		| StatusCode::BAD_GATEWAY
		| StatusCode::SERVICE_UNAVAILABLE
		| StatusCode::GATEWAY_TIMEOUT => Failure::ProviderUnavailable,
		_ => Failure::ProviderRequestRejected,
	}
}

#[async_trait]
impl SearchProvider for BraveSearch {
	fn disclosure(&self, query: &SearchQuery) -> Result<Outbound, Failure> {
		Ok(Outbound {
			destination: self.endpoint.host_str().unwrap_or_default().into(),
			content: self.request(query)?.to_string(),
		})
	}
	fn account(&self) -> Result<Option<AccountTerms>, Failure> {
		self.terms
			.current_at(Utc::now())
			.map_err(|_| Failure::AccountUnavailable)?;
		Ok(Some(self.terms.clone()))
	}
	async fn search(&self, query: &SearchQuery) -> Result<ProviderResults, Failure> {
		let body = self.request(query)?;
		let operation = async {
			let mut response = match self
				.client
				.post(self.endpoint.clone())
				.header("x-subscription-token", self.token.clone())
				.header(header::ACCEPT, "application/json")
				.json(&body)
				.send()
				.await
			{
				Ok(response) => response,
				Err(error) if error.is_timeout() => return Err(Failure::Timeout),
				Err(_) => return Err(Failure::TransportError),
			};
			if !response.status().is_success() {
				return Err(status_failure(response.status()));
			}
			let mut bytes = Vec::new();
			loop {
				let chunk = response
					.chunk()
					.await
					.map_err(|_| Failure::ResponseStreamError)?;
				let Some(chunk) = chunk else { break };
				if bytes.len().saturating_add(chunk.len()) > MAX_PROVIDER_BYTES {
					return Err(Failure::ResponseTooLarge);
				}
				bytes.extend_from_slice(&chunk);
			}
			parse_response(&bytes)
		};
		tokio::time::timeout(TIMEOUT, operation)
			.await
			.unwrap_or(Err(Failure::Timeout))
	}
}

#[cfg(test)]
mod tests;
