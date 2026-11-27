//! Bounds and pagination shared by MCP listing transports.
use crate::ports::tools::ListFailure;
use serde::{Deserialize, Serialize};
use std::{future::Future, io, time::Duration};

/// Bounds the time a listing can occupy a session, including empty pages.
pub const MCP_LIST_TIMEOUT: Duration = Duration::from_secs(15);
/// Protect server memory against an untrusted MCP server's listing.
pub const MAX_LISTED_SCHEMA_BYTES: usize = 1 << 20;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
	pub tools: Vec<T>,
	pub next_cursor: Option<String>,
}

/// Count encoded bytes without allocating a second copy of the page.
struct Remaining(usize);

impl io::Write for Remaining {
	fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
		self.0 = self
			.0
			.checked_sub(bytes.len())
			.ok_or_else(|| io::Error::other(ListFailure::ListingTooLarge))?;
		Ok(bytes.len())
	}

	fn flush(&mut self) -> io::Result<()> {
		Ok(())
	}
}

/// Collect the complete listing. Count all retained metadata, including descriptions.
pub async fn list<T, E, F>(mut fetch: impl FnMut(Option<String>) -> F) -> Result<Vec<T>, E>
where
	T: Serialize,
	E: From<ListFailure>,
	F: Future<Output = Result<Page<T>, E>>,
{
	tokio::time::timeout(MCP_LIST_TIMEOUT, async {
		let mut tools = Vec::new();
		let mut cursor = None;
		let mut remaining = Remaining(MAX_LISTED_SCHEMA_BYTES);
		loop {
			let page = fetch(cursor).await?;
			serde_json::to_writer(&mut remaining, &page).map_err(|error| {
				if error.is_io() {
					ListFailure::ListingTooLarge
				} else {
					ListFailure::InvalidResponse
				}
			})?;
			tools.extend(page.tools);
			cursor = page.next_cursor;
			if cursor.is_none() {
				return Ok(tools);
			}
		}
	})
	.await
	.map_err(|_| ListFailure::ListingTimeout)?
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn lists_beyond_all_former_page_counts_and_stops_at_the_final_page() {
		let tools = list::<_, ListFailure, _>(|cursor| async move {
			let page: usize = cursor.map_or(0, |value| value.parse().unwrap());
			Ok(Page {
				tools: vec![page],
				next_cursor: (page < 24).then(|| (page + 1).to_string()),
			})
		})
		.await
		.unwrap();
		assert_eq!(tools, (0..25).collect::<Vec<_>>());
	}

	#[tokio::test(start_paused = true)]
	async fn listing_deadline_covers_all_pages() {
		let result = list::<_, ListFailure, _>(|cursor| async move {
			tokio::time::sleep(Duration::from_secs(8)).await;
			Ok(Page {
				tools: vec!["tool"],
				next_cursor: cursor.is_none().then(|| "next".into()),
			})
		})
		.await;
		assert_eq!(result, Err(ListFailure::ListingTimeout));
	}
}
