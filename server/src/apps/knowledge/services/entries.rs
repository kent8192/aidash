use crate::Result;
use crate::apps::knowledge::models::cleanup;
use crate::apps::knowledge::*;
use crate::{authorization::identity::Actor, federation::Federation};
use reinhardt::injectable;
use uuid::Uuid;

#[derive(Clone)]
pub struct SemanticEntries {
	runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> SemanticEntries {
	SemanticEntries { runtime }
}

impl SemanticEntries {
	pub async fn configure(&self, workspace: Uuid, input: ConfigureIndex) -> Result<Index> {
		let f = self.runtime.clone();
		service::configure(&f.store, workspace, input).await
	}
	pub async fn index(&self, actor: Actor, workspace: Uuid) -> Result<Index> {
		let f = self.runtime.clone();
		service::get_index(&f.store, &actor, workspace).await
	}
	pub async fn put(&self, actor: Actor, workspace: Uuid, input: PutEntry) -> Result<Entry> {
		let f = self.runtime.clone();
		service::put(&f.store, &actor, workspace, input).await
	}
	pub async fn entries(&self, actor: Actor, workspace: Uuid) -> Result<Vec<Entry>> {
		let f = self.runtime.clone();
		service::entries(&f.store, &actor, workspace).await
	}
	pub async fn delete(
		&self,
		actor: Actor,
		(workspace, id): (Uuid, Uuid),
		input: Revision,
	) -> Result<Entry> {
		let f = self.runtime.clone();
		service::change(
			&f.store,
			&actor,
			workspace,
			id,
			input.expected_revision,
			true,
		)
		.await
	}
	pub async fn reindex(
		&self,
		actor: Actor,
		(workspace, id): (Uuid, Uuid),
		input: Revision,
	) -> Result<Entry> {
		let f = self.runtime.clone();
		service::change(
			&f.store,
			&actor,
			workspace,
			id,
			input.expected_revision,
			false,
		)
		.await
	}
	pub async fn search(
		&self,
		actor: Actor,
		workspace: Uuid,
		input: Search,
	) -> Result<SearchResult> {
		let f = self.runtime.clone();
		// Reserve effect/audit capacity independently from API revokers waiting on
		// this search's credential and policy lease.
		static SEARCH_CAPACITY: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
		let _permit = SEARCH_CAPACITY
			.acquire()
			.await
			.expect("search capacity stays open");
		let worker = f.for_workers().await?;
		let result = service::search(&worker.store, &actor, workspace, input).await;
		worker.store.pool.close().await;
		worker.store.control_pool.close().await;
		result
	}
	pub async fn history(&self, actor: Actor, workspace: Uuid) -> Result<Vec<History>> {
		let f = self.runtime.clone();
		service::history_list(&f.store, &actor, workspace).await
	}
	pub async fn cleanup(&self, workspace: Uuid) -> Result<CleanupStatus> {
		cleanup::status(&self.runtime.store.database(), workspace).await
	}
}
