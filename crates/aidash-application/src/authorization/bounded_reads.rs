//! Bounded candidate pages retain current disclosure decisions.
pub use crate::ports::authorization::bounded_reads::HumanState;
use crate::{Result, ports::authorization::bounded_reads::Scope};
use aidash_domain::{
	HumanRequest, RunInspection,
	unconfirmed::{Kind, Tally, Unconfirmed},
};
use serde::Serialize;
use std::collections::HashMap;
use uuid::Uuid;

pub const PAGE_SIZE: usize = 50;

#[derive(Serialize, schemars::JsonSchema)]
pub struct RunPage {
	pub items: Vec<RunInspection>,
	/// Opaque continuation cursor.
	pub next_cursor: Option<String>,
	/// Runs of this page's candidates that could not be confirmed because a
	/// Node did not answer, for each Node.
	pub unconfirmed: Vec<Unconfirmed>,
}
#[derive(Serialize, schemars::JsonSchema)]
pub struct HumanPage {
	pub items: Vec<HumanRequest>,
	/// Opaque continuation cursor.
	pub next_cursor: Option<String>,
	/// Requests of this page's candidates that could not be confirmed because a
	/// Node did not answer, for each Node.
	pub unconfirmed: Vec<Unconfirmed>,
}

/// Pages contain at most 50 candidate rows. Denied rows are skipped, so a
/// short or empty page can still have a cursor to the next candidate page.
pub async fn runs(scope: &mut dyn Scope, task: Uuid, cursor: Option<Uuid>) -> Result<RunPage> {
	scope.require_task(task).await?;
	let rows = scope.runs(task, cursor).await?;
	let next_cursor =
		(rows.len() == PAGE_SIZE).then(|| rows.last().unwrap().metadata.id.to_string());
	let mut items = Vec::new();
	let mut unconfirmed = Tally::new();
	for row in rows {
		scope.begin_row();
		let run = scope.visible_run(row.metadata.id).await?;
		let silence = scope.take_row_silence();
		let workspace = row.metadata.workspace_id;
		match run {
			Some(run) => items.push(run),
			None => {
				unconfirmed.settle(silence, Kind::Run, workspace, false);
			}
		}
	}
	Ok(RunPage {
		items,
		next_cursor,
		unconfirmed: unconfirmed.into_entries(),
	})
}

pub async fn humans(
	scope: &mut dyn Scope,
	run: Option<Uuid>,
	state: HumanState,
	cursor: Option<Uuid>,
) -> Result<HumanPage> {
	let mut visible = HashMap::new();
	if let Some(id) = run {
		visible.insert(id, Some(scope.require_run(id).await?));
	}
	let rows = scope.humans(run, state, cursor).await?;
	let next_cursor = (rows.len() == PAGE_SIZE).then(|| rows.last().unwrap().id.to_string());
	let mut items = Vec::new();
	let mut unconfirmed = Tally::new();
	// The Node each hidden Run's check gave up on, for the requests that follow it.
	let mut silent: HashMap<Uuid, String> = HashMap::new();
	for mut request in rows {
		if let std::collections::hash_map::Entry::Vacant(entry) = visible.entry(request.run_id) {
			scope.begin_row();
			let run = scope.visible_run(request.run_id).await?;
			if let (None, Some(node)) = (&run, scope.take_row_silence()) {
				silent.insert(request.run_id, node);
			}
			entry.insert(run);
		}
		let Some(run) = visible.get(&request.run_id).and_then(Option::as_ref) else {
			// The reader's own `human.read` comes first, so a request they may not
			// read is not counted as one they would see (#770).
			if let Some(node) = silent.get(&request.run_id) {
				let resource = scope.human_resource(&request).await?;
				if scope.decide(&resource, "human.read").await? {
					unconfirmed.record(node, Kind::HumanRequest, request.workspace_id);
				}
			}
			continue;
		};
		let resource = scope.human_resource(&request).await?;
		if scope.decide(&resource, "human.read").await? {
			request.answer_by = super::state::answer_deadlines(std::slice::from_ref(run))
				.get(&request.id)
				.copied()
				.filter(|_| request.response.is_none());
			items.push(request);
		}
	}
	Ok(HumanPage {
		items,
		next_cursor,
		unconfirmed: unconfirmed.into_entries(),
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidash_domain::{RawRun, RunControl, RunMetadata, RunPhase, policy::Resource};
	use async_trait::async_trait;
	use serde_json::json;

	struct Hidden {
		seen: usize,
	}
	impl crate::ports::authorization::visibility::RowOutcome for Hidden {}
	#[async_trait]
	impl Scope for Hidden {
		async fn require_task(&mut self, _: Uuid) -> Result<()> {
			Ok(())
		}
		async fn require_run(&mut self, _: Uuid) -> Result<RunInspection> {
			unreachable!()
		}
		async fn visible_run(&mut self, _: Uuid) -> Result<Option<RunInspection>> {
			self.seen += 1;
			Ok(None)
		}
		async fn runs(&mut self, _: Uuid, _: Option<Uuid>) -> Result<Vec<RawRun>> {
			Ok((1..=50)
				.map(|id| RawRun {
					metadata: RunMetadata {
						id: Uuid::from_u128(id),
						task_id: Uuid::from_u128(90),
						workspace_id: Uuid::from_u128(91),
						node: "brave-otter-k3m9q2xa".parse().unwrap(),
						home_node: "aidash://local/brave-otter-k3m9q2xa".into(),
						agent_id: "agent".into(),
						agent_version: "1".into(),
						phase: RunPhase::Ready,
						control: RunControl::Active,
						control_revision: 0,
						control_source: Default::default(),
						step: 0,
						revision: 0,
						observed_input_seq: 0,
						ledger_worker_ready: true,
						error: None,
						lease_owner: None,
						lease_until: None,
						updated_at: chrono::Utc::now(),
					},
					context: json!({}),
					pending: json!({}),
				})
				.collect())
		}
		async fn humans(
			&mut self,
			_: Option<Uuid>,
			_: HumanState,
			_: Option<Uuid>,
		) -> Result<Vec<HumanRequest>> {
			unreachable!()
		}
		async fn human_resource(&mut self, _: &HumanRequest) -> Result<Resource> {
			unreachable!()
		}
		async fn decide(&mut self, _: &Resource, _: &str) -> Result<bool> {
			unreachable!()
		}
	}

	#[tokio::test]
	async fn fully_hidden_candidate_page_still_advances_without_scanning_another_page() {
		let mut scope = Hidden { seen: 0 };
		let page = runs(&mut scope, Uuid::from_u128(90), None).await.unwrap();
		assert!(page.items.is_empty());
		assert_eq!(page.next_cursor, Some(Uuid::from_u128(50).to_string()));
		assert_eq!(scope.seen, 50);
	}

	struct Requests {
		run: RunInspection,
		calls: usize,
		deadline: chrono::DateTime<chrono::Utc>,
	}
	impl crate::ports::authorization::visibility::RowOutcome for Requests {}
	#[async_trait]
	impl Scope for Requests {
		async fn require_task(&mut self, _: Uuid) -> Result<()> {
			unreachable!()
		}
		async fn require_run(&mut self, _: Uuid) -> Result<RunInspection> {
			self.calls += 1;
			Ok(self.run.clone())
		}
		async fn visible_run(&mut self, _: Uuid) -> Result<Option<RunInspection>> {
			self.calls += 1;
			Ok(Some(self.run.clone()))
		}
		async fn runs(&mut self, _: Uuid, _: Option<Uuid>) -> Result<Vec<RawRun>> {
			unreachable!()
		}
		async fn humans(
			&mut self,
			_: Option<Uuid>,
			_: HumanState,
			_: Option<Uuid>,
		) -> Result<Vec<HumanRequest>> {
			Ok((1..=2)
				.map(|id| HumanRequest {
					id: Uuid::from_u128(id),
					workspace_id: self.run.workspace_id,
					run_id: self.run.id,
					kind: "QUESTION".into(),
					prompt: "Question".into(),
					response: None,
					created_at: self.deadline,
					answered_by: None,
					answer_by: None,
				})
				.collect())
		}
		async fn human_resource(&mut self, request: &HumanRequest) -> Result<Resource> {
			Ok(Resource {
				tenant: "tenant".into(),
				kind: "human".into(),
				id: request.id.to_string(),
				attributes: json!({}),
			})
		}
		async fn decide(&mut self, _: &Resource, _: &str) -> Result<bool> {
			Ok(true)
		}
	}

	#[tokio::test]
	async fn requests_of_one_run_share_visibility_and_only_the_waited_request_has_a_deadline() {
		use aidash_domain::{ReadyState, ResumeState, RunState, WaitingState};
		let raw = Hidden { seen: 0 }
			.runs(Uuid::nil(), None)
			.await
			.unwrap()
			.remove(0);
		let mut run = raw.inspect();
		let deadline = chrono::DateTime::from_timestamp(1_900_000_000, 0).unwrap();
		run.state = Some(RunState::Waiting(Box::new(WaitingState::Human {
			request_id: Uuid::from_u128(1),
			answer_by: Some(deadline),
			resume: ResumeState::Ready(ReadyState {}),
		})));
		let mut scope = Requests {
			run,
			calls: 0,
			deadline,
		};
		let run_id = scope.run.id;
		let page = humans(&mut scope, Some(run_id), HumanState::Pending, None)
			.await
			.unwrap();
		assert_eq!(scope.calls, 1);
		assert_eq!(page.items.len(), 2);
		assert_eq!(page.items[0].answer_by, Some(deadline));
		assert_eq!(page.items[1].answer_by, None);
	}

	/// Runs 1..=3 stop at a Node that did not answer, 4..=8 are refused, and the
	/// requests of Run 1 are two, one of which the reader may not read (#770).
	struct Mixed {
		silence: Option<String>,
	}
	impl crate::ports::authorization::visibility::RowOutcome for Mixed {
		fn begin_row(&mut self) {
			self.silence = None;
		}
		fn row_silence(&self) -> Option<String> {
			self.silence.clone()
		}
		fn take_row_silence(&mut self) -> Option<String> {
			self.silence.take()
		}
	}
	#[async_trait]
	impl Scope for Mixed {
		async fn require_task(&mut self, _: Uuid) -> Result<()> {
			Ok(())
		}
		async fn require_run(&mut self, _: Uuid) -> Result<RunInspection> {
			unreachable!()
		}
		async fn visible_run(&mut self, id: Uuid) -> Result<Option<RunInspection>> {
			if (1..=3).contains(&id.as_u128()) {
				self.silence = Some("aidash://other/silent".into());
			}
			Ok(None)
		}
		async fn runs(&mut self, task: Uuid, cursor: Option<Uuid>) -> Result<Vec<RawRun>> {
			Hidden { seen: 0 }.runs(task, cursor).await.map(|mut rows| {
				rows.truncate(8);
				rows
			})
		}
		async fn humans(
			&mut self,
			_: Option<Uuid>,
			_: HumanState,
			_: Option<Uuid>,
		) -> Result<Vec<HumanRequest>> {
			let request = |id: u128, prompt: &str| HumanRequest {
				id: Uuid::from_u128(id),
				workspace_id: Uuid::from_u128(91),
				run_id: Uuid::from_u128(1),
				kind: "QUESTION".into(),
				prompt: prompt.into(),
				response: None,
				created_at: chrono::Utc::now(),
				answered_by: None,
				answer_by: None,
			};
			Ok(vec![request(70, "readable"), request(71, "hidden")])
		}
		async fn human_resource(&mut self, request: &HumanRequest) -> Result<Resource> {
			Ok(Resource {
				tenant: "tenant".into(),
				kind: "human".into(),
				id: request.id.to_string(),
				attributes: json!({"prompt": request.prompt}),
			})
		}
		async fn decide(&mut self, resource: &Resource, _: &str) -> Result<bool> {
			Ok(resource.attributes["prompt"] != "hidden")
		}
	}

	#[tokio::test]
	async fn three_unconfirmed_runs_and_five_refusals_report_three() {
		let mut scope = Mixed { silence: None };
		let page = runs(&mut scope, Uuid::from_u128(90), None).await.unwrap();
		assert!(page.items.is_empty());
		assert_eq!(
			page.unconfirmed,
			vec![Unconfirmed {
				node: "aidash://other/silent".into(),
				kind: Kind::Run,
				workspace_id: Uuid::from_u128(91),
				count: 3,
			}]
		);
	}

	#[tokio::test]
	async fn requests_of_a_run_that_cannot_be_confirmed_are_counted_after_human_read() {
		let mut scope = Mixed { silence: None };
		let page = humans(&mut scope, None, HumanState::All, None)
			.await
			.unwrap();
		assert!(page.items.is_empty());
		// Only the request the reader may read counts, once, under the silent Node.
		assert_eq!(
			page.unconfirmed,
			vec![Unconfirmed {
				node: "aidash://other/silent".into(),
				kind: Kind::HumanRequest,
				workspace_id: Uuid::from_u128(91),
				count: 1,
			}]
		);
	}
}
