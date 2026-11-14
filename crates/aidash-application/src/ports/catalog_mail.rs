//! A request notice contains only the information an approver needs to review it.
use async_trait::async_trait;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogRequestNotice {
	pub definition: aidash_domain::registry::EntityRef,
	pub node: aidash_domain::nodes::NodeName,
	pub reason: String,
	pub review_link: String,
}

/// Mail delivery is an external effect and must be called after the request commits.
#[async_trait]
pub trait CatalogRequestMailer: Send + Sync {
	async fn send(&self, recipient: &str, notice: &CatalogRequestNotice) -> crate::Result<()>;
}
