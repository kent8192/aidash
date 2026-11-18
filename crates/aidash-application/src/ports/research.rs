//! The port page reading reads through. Aidash's own reader implements it.
use aidash_domain::research::{Failure, document::ExtractedPage};
use async_trait::async_trait;
use url::Url;

/// A Page reading provider. It must refuse non-public destinations itself,
/// including addresses a public name resolves to at dispatch.
#[async_trait]
pub trait PageProvider: Send + Sync {
	/// Literal address checks performed before anything is sent.
	fn admit(&self, url: &Url) -> std::result::Result<(), Failure>;
	async fn fetch(&self, url: &Url) -> std::result::Result<ExtractedPage, Failure>;
}
