//! A deterministic research provider over an in-memory corpus. It implements
//! the same search and page contracts as the Brave and page packages, so it
//! replaces them in acceptance tests without any configuration change. It never
//! sends anything outside the Node.
use super::admit_public_url;
use crate::research::Failure;
use aidash_application::ports::research::{Outbound, PageProvider, PageRenderer, SearchProvider};
use aidash_domain::research::{
	account::AccountTerms,
	document::{Completeness, ExtractedPage, Line, RenderedPage, fold},
	search::{ProviderCandidate, ProviderResults, SearchQuery},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Url;
use sha2::{Digest, Sha256};

/// The destination named in fixture disclosures; it is never contacted.
pub const DESTINATION: &str = "research-fixture.invalid";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixturePage {
	pub url: String,
	pub title: String,
	pub lines: Vec<Line>,
	/// Set when the page is a PDF.
	pub pdf: Option<FixturePdf>,
}

/// A fixture page that is a PDF: its page count, the pages that carry no text
/// and the document itself, which the host retains to view those pages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixturePdf {
	pub page_count: u32,
	pub pages_without_text: Vec<u32>,
	pub original: Vec<u8>,
}

/// A three-page PDF generated with Ghostscript: text on pages 1 and 3, and on
/// page 2 only filled shapes, standing in for a scanned chart.
pub const ANNUAL_REPORT_PDF: &[u8] = include_bytes!("pdf/fixtures/annual-report.pdf");

pub struct FixtureResearch {
	pages: Vec<FixturePage>,
	fetched_at: DateTime<Utc>,
}

impl FixtureResearch {
	pub fn new(pages: Vec<FixturePage>) -> Self {
		Self {
			pages,
			fetched_at: "2026-10-01T00:00:00Z".parse().expect("fixed fixture time"),
		}
	}
	fn text(page: &FixturePage) -> String {
		fold(
			&std::iter::once(page.title.as_str())
				.chain(page.lines.iter().map(|line| line.text.as_str()))
				.collect::<Vec<_>>()
				.join("\n"),
		)
	}
}

impl Default for FixtureResearch {
	/// A small public-looking corpus for local evaluation and examples.
	fn default() -> Self {
		let line = |text: &str, page| Line {
			text: text.into(),
			page,
		};
		Self::new(vec![
			FixturePage {
				url: "https://research-fixture.example/rust-release".into(),
				title: "Rust release notes".into(),
				lines: vec![
					line("Rust 1.96 stabilizes several standard library APIs.", None),
					line(
						"The release keeps the 2024 edition as the default for new projects.",
						None,
					),
				],
				pdf: None,
			},
			FixturePage {
				url: "https://research-fixture.example/strasse".into(),
				title: "Die Straße".into(),
				lines: vec![
					line("Alpha line", None),
					line("Die Straße <script>inert text</script>", Some(2)),
					line("Omega line", Some(2)),
				],
				pdf: None,
			},
			FixturePage {
				url: "https://research-fixture.example/annual-report.pdf".into(),
				title: "Annual report".into(),
				lines: vec![
					line("Annual report: revenue grew by twelve percent.", Some(1)),
					line("Operating costs fell for the third year.", Some(1)),
					line(
						"Appendix: the scanned chart on page two is not text.",
						Some(3),
					),
				],
				pdf: Some(FixturePdf {
					page_count: 3,
					pages_without_text: vec![2],
					original: ANNUAL_REPORT_PDF.to_vec(),
				}),
			},
		])
	}
}

#[async_trait]
impl SearchProvider for FixtureResearch {
	fn disclosure(&self, query: &SearchQuery) -> Result<Outbound, Failure> {
		Ok(Outbound {
			destination: DESTINATION.into(),
			content: query.query.clone(),
		})
	}
	fn account(&self) -> Result<Option<AccountTerms>, Failure> {
		Ok(None)
	}
	async fn search(&self, query: &SearchQuery) -> Result<ProviderResults, Failure> {
		let terms = fold(&query.query)
			.split_whitespace()
			.map(str::to_owned)
			.collect::<Vec<_>>();
		let mut matches = self
			.pages
			.iter()
			.map(|page| {
				let text = Self::text(page);
				(
					terms.iter().filter(|term| text.contains(*term)).count(),
					page,
				)
			})
			.filter(|(score, _)| *score > 0)
			.collect::<Vec<_>>();
		matches.sort_by(|(a, left), (b, right)| b.cmp(a).then(left.url.cmp(&right.url)));
		let start = usize::from(query.page) * usize::from(query.count);
		Ok(ProviderResults {
			more_results_available: matches.len() > start + usize::from(query.count),
			candidates: matches
				.into_iter()
				.skip(start)
				.map(|(_, page)| ProviderCandidate {
					url: page.url.clone(),
					title: page.title.clone(),
					snippet: page.lines.first().map(|line| line.text.clone()),
					page_age: None,
				})
				.collect(),
		})
	}
}

#[async_trait]
impl PageProvider for FixtureResearch {
	fn admit(&self, url: &Url) -> Result<(), Failure> {
		admit_public_url(url)
	}
	async fn fetch(&self, url: &Url) -> Result<ExtractedPage, Failure> {
		let page = self
			.pages
			.iter()
			.find(|page| Url::parse(&page.url).is_ok_and(|candidate| candidate == *url))
			.ok_or(Failure::ProviderRequestRejected)?;
		let raw = match &page.pdf {
			Some(pdf) => pdf.original.clone(),
			None => serde_json::to_vec(&page.lines).map_err(|_| Failure::ExtractionFailed)?,
		};
		Ok(ExtractedPage {
			url: page.url.clone(),
			title: Some(page.title.clone()),
			media_type: if page.pdf.is_some() {
				"application/pdf"
			} else {
				"text/plain"
			}
			.into(),
			fetched_at: self.fetched_at,
			raw_digest: format!("sha256:{:x}", Sha256::digest(&raw)),
			completeness: Completeness::Complete,
			lines: page.lines.clone(),
			page_count: page.pdf.as_ref().map(|pdf| pdf.page_count),
			pages_without_text: page
				.pdf
				.as_ref()
				.map(|pdf| pdf.pages_without_text.clone())
				.unwrap_or_default(),
			original: page
				.pdf
				.as_ref()
				.filter(|pdf| !pdf.pages_without_text.is_empty())
				.map(|pdf| pdf.original.clone()),
		})
	}
}

#[async_trait]
impl PageRenderer for FixtureResearch {
	/// Only the corpus's own documents are rendered, in this process: they are
	/// trusted, so no child is started. Anything else is refused.
	async fn render(&self, pdf: Vec<u8>, page: u32) -> Result<RenderedPage, Failure> {
		if !self.pages.iter().any(|candidate| {
			candidate
				.pdf
				.as_ref()
				.is_some_and(|known| known.original == pdf)
		}) {
			return Err(Failure::RenderFailed);
		}
		super::pdf::render(&pdf, page)
	}
}

#[cfg(test)]
mod tests;
