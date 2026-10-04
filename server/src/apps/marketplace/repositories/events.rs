//! Current policy and event reads borrow the original Access lease.
use super::definitions::NativeDefinitions;
use aidash_application::ports::marketplace::EventScope;
use aidash_domain::Event;
use async_trait::async_trait;
use uuid::Uuid;
#[async_trait]
impl<Effects: Send> EventScope for NativeDefinitions<'_, Effects> {
	async fn compatibility_ready(&mut self) -> aidash_application::Result<()> {
		crate::apps::marketplace::services::storage::gate(&mut self.access.tx)
			.await
			.map_err(Into::into)
	}
	async fn workspace_allowed(
		&mut self,
		workspace: Uuid,
		action: &str,
	) -> aidash_application::Result<bool> {
		self.access
			.allowed(workspace, action)
			.await
			.map_err(Into::into)
	}
	async fn workspace_event_visible(&mut self, event: &Event) -> aidash_application::Result<bool> {
		self.access.event_visible(event).await.map_err(Into::into)
	}
}
