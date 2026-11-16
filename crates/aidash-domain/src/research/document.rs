//! The readable text a page reader extracts from one retrieval of a public
//! Web page. Page reading keeps none of it: each call reads the page again
//! and returns one part of its text (#987).
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Text read per page, in UTF-8 bytes.
pub const MAX_SNAPSHOT_BYTES: usize = 512 * 1024;
pub const MAX_LINES: usize = 20_000;
pub const MAX_LINE_BYTES: usize = 16 * 1024;
/// The largest original a reader may pass along with a PDF.
pub const MAX_ORIGINAL_BYTES: usize = 16 * 1024 * 1024;
/// The largest page count a snapshot records, the range its page numbers are
/// stored in.
pub const MAX_PAGES: u32 = i32::MAX as u32;
/// The most pages a snapshot lists as carrying no text. Page reading reads
/// no more pages than this, and the list, like the rest of a result, must fit
/// within [`super::MAX_RESULT_BYTES`].
pub const MAX_PAGES_WITHOUT_TEXT: usize = 500;
/// Bytes of page text one call returns as one part.
pub const MAX_PART_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Line {
	pub text: String,
	/// 1-based page of a paginated source such as a PDF.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub page: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Completeness {
	Complete,
	/// The page had more text than a snapshot retains.
	TextLimit,
}

impl Completeness {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Complete => "complete",
			Self::TextLimit => "text_limit",
		}
	}
}

/// Readable text a page provider extracted from one retrieval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractedPage {
	/// The address that answered, after any permitted redirect.
	pub url: String,
	pub title: Option<String>,
	pub media_type: String,
	pub fetched_at: DateTime<Utc>,
	/// `sha256:` digest of the retrieved bytes.
	pub raw_digest: String,
	pub completeness: Completeness,
	pub lines: Vec<Line>,
	/// The page count of a paginated source such as a PDF.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub page_count: Option<u32>,
	/// 1-based pages that carry no text, in page order.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub pages_without_text: Vec<u32>,
	/// The retrieved document, present only when some page carries no text, so
	/// that the Run's model can view that page (ADR 0053).
	#[serde(skip)]
	pub original: Option<Vec<u8>>,
}

impl ExtractedPage {
	/// Provider output is untrusted; the host enforces the snapshot bounds.
	/// A paginated source may have no text lines when its pages carry none.
	pub fn validate(&self) -> Result<()> {
		let bytes = self.lines.iter().map(|line| line.text.len()).sum::<usize>();
		let within = |page: u32| page > 0 && self.page_count.is_none_or(|count| page <= count);
		if (self.lines.is_empty() && self.pages_without_text.is_empty())
			|| self.page_count == Some(0)
			|| self.page_count.is_some_and(|count| count > MAX_PAGES)
			|| self.lines.len() > MAX_LINES
			|| self
				.lines
				.iter()
				.any(|line| line.page.is_some_and(|page| !within(page)))
			|| (!self.pages_without_text.is_empty() && self.page_count.is_none())
			|| self.pages_without_text.len() > MAX_PAGES_WITHOUT_TEXT
			|| !self.pages_without_text.iter().all(|page| within(*page))
			|| !self
				.pages_without_text
				.windows(2)
				.all(|pair| pair[0] < pair[1])
			// A page either carries text or is listed as carrying none. The
			// list is sorted by the check above.
			|| self.lines.iter().any(|line| {
				line.page
					.is_some_and(|page| self.pages_without_text.binary_search(&page).is_ok())
			})
			// A complete paginated source accounts for every page.
			|| (self.completeness == Completeness::Complete
				&& self.page_count.is_some_and(|count| !self.covers(count)))
			// The original travels exactly when some page carries no text; the
			// host, not the provider, decides whether it is retained.
			|| match &self.original {
				Some(original) => {
					self.pages_without_text.is_empty()
						|| original.is_empty()
						|| original.len() > MAX_ORIGINAL_BYTES
				}
				None => !self.pages_without_text.is_empty(),
			} || bytes > MAX_SNAPSHOT_BYTES
			|| self.lines.iter().any(|line| {
				line.text.len() > MAX_LINE_BYTES
					|| line.page == Some(0)
					|| line.text.chars().any(|c| c.is_control() && c != '\t')
			}) || self
			.title
			.as_ref()
			.is_some_and(|title| title.len() > 512 || title.chars().any(char::is_control))
			|| super::address::candidate_host(&self.url).is_none()
			|| !self.raw_digest.starts_with("sha256:")
		{
			return Err(Error::Invalid(
				"extracted page violates snapshot bounds".into(),
			));
		}
		Ok(())
	}

	/// Whether every page through `count` carries text or is listed as
	/// carrying none. [`Self::validate`] keeps every page it names within
	/// `count`, so counting the distinct pages suffices.
	fn covers(&self, count: u32) -> bool {
		let mut pages = self
			.lines
			.iter()
			.filter_map(|line| line.page)
			.collect::<std::collections::BTreeSet<_>>();
		pages.extend(&self.pages_without_text);
		usize::try_from(count).is_ok_and(|count| pages.len() == count)
	}
}
