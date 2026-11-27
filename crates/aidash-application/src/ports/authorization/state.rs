//! Authorized state pages share one live policy snapshot and transaction.
use super::visibility::RowOutcome;
use crate::Result;
use crate::authorization::projection::EventRead;
use aidash_domain::{
	Artifact, Conversation, HumanRequest, RunInspection, RunMetadata, Task, Thread,
	policy::Resource, registry::Entry, run_state::RawRun, threads::TaskPage,
};
use async_trait::async_trait;
use uuid::Uuid;
/// Visible Runs by id (`None` when hidden), and the Node each hidden Run's
/// check gave up on because it did not answer (#770).
pub type VisibleRuns = (
	std::collections::HashMap<Uuid, Option<RunInspection>>,
	std::collections::HashMap<Uuid, String>,
);
#[async_trait]
pub trait ThreadListingScope: Send + RowOutcome {
	async fn visible(&mut self, action: &str) -> Result<Vec<Uuid>>;
	async fn task_page(&mut self, threads: &[Uuid], offset: u64) -> Result<TaskPage>;
}
#[async_trait]
pub trait ThreadStateScope: Send + RowOutcome {
	async fn allowed(&mut self, thread: Uuid, action: &str) -> Result<bool>;
	async fn prepare_threads(&mut self, _threads: &[Uuid]) -> Result<()> {
		Ok(())
	}
	async fn registry(&mut self) -> Result<Vec<Entry>>;
	async fn thread_rows(&mut self, threads: &[Uuid]) -> Result<Vec<Thread>>;
	/// Candidate pages contain at most PAGE_SIZE rows, ordered newest first.
	async fn thread_candidates(
		&mut self,
		node: Option<&str>,
		cursor: Option<Uuid>,
	) -> Result<Vec<Uuid>>;
	async fn task_candidates(&mut self, node: Option<&str>) -> Result<Vec<Task>>;
	async fn artifact_candidates(&mut self, node: Option<&str>) -> Result<Vec<Artifact>>;
	async fn run_candidates(&mut self, node: Option<&str>) -> Result<Vec<RawRun>>;
	async fn pending_human_candidates(&mut self, node: Option<&str>) -> Result<Vec<HumanRequest>>;
	async fn conversation_candidates(&mut self, node: Option<&str>) -> Result<Vec<Conversation>>;
	/// The latest visible events, and the events left out because a Node did
	/// not answer (#770).
	async fn latest_visible_events(&mut self, node: Option<&str>, before: i64)
	-> Result<EventRead>;
	async fn prepare_tasks(&mut self, _tasks: &[Task]) -> Result<()> {
		Ok(())
	}
	async fn prepare_artifacts(&mut self, _artifacts: &[Artifact]) -> Result<()> {
		Ok(())
	}
	async fn task_visible(&mut self, task: &Task) -> Result<bool>;
	async fn artifact_visible(&mut self, artifact: &Artifact) -> Result<bool>;
	/// Prepare candidate rows for the state window without changing visibility rules.
	async fn prepare_runs(&mut self, _runs: &[RunMetadata]) -> Result<()> {
		Ok(())
	}
	async fn run_visible(&mut self, run: &RunMetadata) -> Result<bool>;
	async fn visible_run(&mut self, id: Uuid) -> Result<Option<RunInspection>>;
	/// [`Self::visible_run`] for each of `ids`, decided together where the adapter
	/// can, so the reads do not grow with the number of Runs. Beside them, the
	/// Node each hidden Run's check gave up on because it did not answer (#770).
	async fn visible_runs(&mut self, ids: &[Uuid]) -> Result<VisibleRuns> {
		let mut runs = std::collections::HashMap::with_capacity(ids.len());
		let mut silent = std::collections::HashMap::new();
		for id in ids {
			if !runs.contains_key(id) {
				self.begin_row();
				let run = self.visible_run(*id).await?;
				if let (None, Some(node)) = (&run, self.take_row_silence()) {
					silent.insert(*id, node);
				}
				runs.insert(*id, run);
			}
		}
		Ok((runs, silent))
	}
	async fn human_resource(&mut self, request: &HumanRequest) -> Result<Resource>;
	async fn conversation_resource(&mut self, conversation: &Conversation) -> Result<Resource>;
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool>;
}
