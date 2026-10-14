//! Web search input and the host normalization of provider candidates. Search
//! results identify Source records; snippets are discovery hints, not evidence.
use super::bounded;
use crate::{Error, Result};
use chrono::NaiveDate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

pub const MAX_QUERY_CHARS: usize = 600;
pub const MAX_QUERY_WORDS: usize = 75;
pub const MAX_DOMAINS: usize = 5;
pub const MAX_URL_BYTES: usize = 4096;
const MAX_INPUT_BYTES: usize = 4096;
const MAX_TITLE_BYTES: usize = 512;
const MAX_SNIPPET_BYTES: usize = 1024;
const MAX_PAGE_AGE_BYTES: usize = 128;

/// Two-letter regions every search package of contract 1 must accept.
pub const COUNTRIES: &[&str] = &[
	"AR", "AU", "AT", "BE", "BR", "CA", "CL", "DK", "FI", "FR", "DE", "HK", "IN", "ID", "IT", "JP",
	"KR", "MY", "MX", "NL", "NZ", "NO", "CN", "PL", "PT", "PH", "RU", "SA", "ZA", "ES", "SE", "CH",
	"TW", "TR", "GB", "US",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Language {
	Ja,
	En,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Period {
	Day,
	Week,
	Month,
	Year,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DateRange {
	pub from: String,
	pub to: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum Freshness {
	Named(Period),
	Range(DateRange),
}

/// The Agent-facing search arguments of research contract 1.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchInput {
	/// Search terms, at most 600 characters and 75 words.
	pub query: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub language: Option<Language>,
	/// Two-letter uppercase region code.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub country: Option<String>,
	/// Number of results, 1–10 (default 5).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub count: Option<u8>,
	/// Result page, 0–9 (default 0).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub page: Option<u8>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub freshness: Option<Freshness>,
	/// Restrict results to these domains and their subdomains.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub include_domains: Vec<String>,
	/// Remove results from these domains and their subdomains.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub exclude_domains: Vec<String>,
}

/// A validated search. Providers translate it; they never relax its filters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
	pub query: String,
	pub language: Language,
	pub country: String,
	pub count: u8,
	pub page: u8,
	pub freshness: Option<ValidFreshness>,
	pub include_domains: Vec<String>,
	pub exclude_domains: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidFreshness {
	Period(Period),
	Range { from: NaiveDate, to: NaiveDate },
}

impl SearchQuery {
	/// An explicit language takes precedence over the Agent's supported
	/// preference; without either, English is used.
	pub fn parse(input: &Value, preferred_language: Option<Language>) -> Result<Self> {
		if serde_json::to_vec(input)?.len() > MAX_INPUT_BYTES {
			return Err(Error::Invalid("search input exceeds 4 KiB".into()));
		}
		let input: SearchInput = serde_json::from_value(input.clone())
			.map_err(|_| Error::Invalid("invalid search input".into()))?;
		let query = input.query.trim();
		if query.is_empty() || query.chars().any(char::is_control) || !valid_query(query) {
			return Err(Error::Invalid("invalid search query".into()));
		}
		let count = input.count.unwrap_or(5);
		let page = input.page.unwrap_or(0);
		if !(1..=10).contains(&count) || page > 9 {
			return Err(Error::Invalid("invalid search count or page".into()));
		}
		let country = input.country.unwrap_or_else(|| "US".into());
		if !COUNTRIES.contains(&country.as_str()) {
			return Err(Error::Invalid("invalid search country".into()));
		}
		if input.include_domains.len() > MAX_DOMAINS || input.exclude_domains.len() > MAX_DOMAINS {
			return Err(Error::Invalid("too many search domains".into()));
		}
		for domain in input.include_domains.iter().chain(&input.exclude_domains) {
			if !valid_domain(domain) {
				return Err(Error::Invalid("invalid search domain".into()));
			}
		}
		for (index, domain) in input.include_domains.iter().enumerate() {
			if input.include_domains[..index].contains(domain)
				|| input.exclude_domains.iter().any(|excluded| {
					domain_matches(domain, excluded) || domain_matches(excluded, domain)
				}) {
				return Err(Error::Invalid("overlapping search domains".into()));
			}
		}
		for (index, domain) in input.exclude_domains.iter().enumerate() {
			if input.exclude_domains[..index].contains(domain) {
				return Err(Error::Invalid("duplicate search domain".into()));
			}
		}
		let freshness = input.freshness.as_ref().map(freshness).transpose()?;
		Ok(Self {
			query: query.to_owned(),
			language: input
				.language
				.or(preferred_language)
				.unwrap_or(Language::En),
			country,
			count,
			page,
			freshness,
			include_domains: input.include_domains,
			exclude_domains: input.exclude_domains,
		})
	}

	/// Host filtering of untrusted provider candidates: unsafe URLs, domain
	/// filters and size bounds are applied whatever the provider claims.
	pub fn normalize(&self, results: ProviderResults) -> SearchPage {
		let mut page = SearchPage {
			sources: vec![],
			filtered_count: 0,
			truncated: false,
			more_results_available: results.more_results_available,
		};
		for (rank, candidate) in results.candidates.into_iter().enumerate() {
			if page.sources.len() >= usize::from(self.count) {
				page.truncated = true;
				break;
			}
			let Some(host) = candidate_host(&candidate.url) else {
				page.filtered_count += 1;
				continue;
			};
			if (!self.include_domains.is_empty()
				&& !self
					.include_domains
					.iter()
					.any(|domain| domain_matches(&host, domain)))
				|| self
					.exclude_domains
					.iter()
					.any(|domain| domain_matches(&host, domain))
			{
				page.filtered_count += 1;
				continue;
			}
			// Provider metadata is untrusted text: control characters, which
			// stored text cannot always hold (NUL), become spaces.
			let (title, snippet) = (
				inert(&candidate.title),
				inert(candidate.snippet.as_deref().unwrap_or("")),
			);
			let (title, title_cut) = bounded(&title, MAX_TITLE_BYTES);
			let (snippet, snippet_cut) = bounded(&snippet, MAX_SNIPPET_BYTES);
			let (page_age, page_age_cut) =
				candidate.page_age.as_deref().map_or((None, false), |age| {
					let age = inert(age);
					let (age, cut) = bounded(&age, MAX_PAGE_AGE_BYTES);
					(Some(age.to_owned()), cut)
				});
			let truncated = title_cut || snippet_cut || page_age_cut;
			page.truncated |= truncated;
			page.sources.push(SourceCandidate {
				url: candidate.url.clone(),
				title: title.to_owned(),
				snippet: snippet.to_owned(),
				rank: u32::try_from(rank + 1).unwrap_or(u32::MAX),
				page_age,
				truncated,
			});
		}
		page
	}

	/// The effective search as reported to the model and retained with the attempt.
	pub fn effective(&self) -> Value {
		json!({
			"query": self.query,
			"language": self.language,
			"country": self.country,
			"count": self.count,
			"page": self.page,
			"freshness": self.freshness.map(|freshness| match freshness {
				ValidFreshness::Period(period) => json!(period),
				ValidFreshness::Range { from, to } => json!({"from": from, "to": to}),
			}),
			"include_domains": self.include_domains,
			"exclude_domains": self.exclude_domains,
		})
	}
}

/// One raw provider candidate, before host filtering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCandidate {
	pub url: String,
	pub title: String,
	pub snippet: Option<String>,
	pub page_age: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderResults {
	pub candidates: Vec<ProviderCandidate>,
	pub more_results_available: bool,
}

/// A filtered candidate, not yet a Source record of any Run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceCandidate {
	pub url: String,
	pub title: String,
	pub snippet: String,
	pub rank: u32,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub page_age: Option<String>,
	pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchPage {
	pub sources: Vec<SourceCandidate>,
	pub filtered_count: usize,
	pub truncated: bool,
	pub more_results_available: bool,
}

/// Agent-facing JSON schema of [`SearchInput`].
pub fn search_schema() -> Value {
	let domain = json!({"type":"string","minLength":4,"maxLength":253});
	json!({
		"type":"object",
		"required":["query"],
		"properties":{
			"query":{"type":"string","minLength":1,"maxLength":MAX_QUERY_CHARS},
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
			"include_domains":{"type":"array","maxItems":MAX_DOMAINS,"items":domain},
			"exclude_domains":{"type":"array","maxItems":MAX_DOMAINS,"items":domain}
		},
		"additionalProperties":false
	})
}

pub fn valid_query(query: &str) -> bool {
	query.chars().count() <= MAX_QUERY_CHARS && query.split_whitespace().count() <= MAX_QUERY_WORDS
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

/// DNS absolute names keep their root dot in URL hosts; it must not change the
/// restriction applied to the equivalent relative name.
pub fn domain_matches(host: &str, domain: &str) -> bool {
	let host = host.trim_end_matches('.');
	host == domain
		|| host
			.strip_suffix(domain)
			.is_some_and(|prefix| prefix.ends_with('.'))
}

/// The host of an absolute http(s) URL without credentials, fragment or
/// control characters. Whether the host is publicly routable is decided only
/// when a page is read.
pub fn candidate_host(raw: &str) -> Option<String> {
	if raw.len() > MAX_URL_BYTES || raw.chars().any(char::is_control) {
		return None;
	}
	let url = Url::parse(raw).ok()?;
	if !matches!(url.scheme(), "http" | "https")
		|| !url.username().is_empty()
		|| url.password().is_some()
		|| url.fragment().is_some()
	{
		return None;
	}
	url.host_str().map(str::to_owned)
}

/// `text` with every control character replaced by a space.
fn inert(text: &str) -> std::borrow::Cow<'_, str> {
	if text.chars().any(char::is_control) {
		text.chars()
			.map(|c| if c.is_control() { ' ' } else { c })
			.collect::<String>()
			.into()
	} else {
		text.into()
	}
}

fn freshness(value: &Freshness) -> Result<ValidFreshness> {
	match value {
		Freshness::Named(period) => Ok(ValidFreshness::Period(*period)),
		Freshness::Range(range) => {
			let invalid = || Error::Invalid("invalid search date range".into());
			let from = NaiveDate::parse_from_str(&range.from, "%Y-%m-%d").map_err(|_| invalid())?;
			let to = NaiveDate::parse_from_str(&range.to, "%Y-%m-%d").map_err(|_| invalid())?;
			if from > to || from.to_string() != range.from || to.to_string() != range.to {
				return Err(invalid());
			}
			Ok(ValidFreshness::Range { from, to })
		}
	}
}

#[cfg(test)]
mod tests;
