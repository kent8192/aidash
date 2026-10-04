use super::*;
use crate::{
	Error,
	ports::semantic::run_context::{RunSemanticJournal, RunSemanticScope},
};
use aidash_domain::{
	TaskStatus,
	semantic::results::{Match, SearchResult},
};
use async_trait::async_trait;
use chrono::Utc;
use rstest::{fixture, rstest};
use serde_json::json;
use std::sync::{Arc, Mutex};
use uuid::Uuid;
fn id(value: u128) -> Uuid {
	Uuid::from_u128(value)
}
struct State {
	remote: bool,
	local: Option<SearchResult>,
	remote_result: Option<Value>,
	calls: Vec<String>,
	records: Vec<(Uuid, i64)>,
	query: Option<String>,
	budget: Option<usize>,
	fail: Option<&'static str>,
	pending: Option<&'static str>,
}
struct Repository(Arc<Mutex<State>>);
struct Scope(Arc<Mutex<State>>);
struct Journal {
	state: Arc<Mutex<State>>,
	committed: bool,
}
impl State {
	fn record(&mut self, name: &str) -> Result<()> {
		self.calls.push(name.into());
		if self.fail == Some(name) {
			return Err(Error::Port(Box::new(std::io::Error::other(format!(
				"{name} fault"
			)))));
		}
		Ok(())
	}
}
impl Drop for Scope {
	fn drop(&mut self) {
		self.0.lock().unwrap().calls.push("source_release".into());
	}
}
impl Drop for Journal {
	fn drop(&mut self) {
		if !self.committed {
			self.state
				.lock()
				.unwrap()
				.calls
				.push("journal_rollback".into());
		}
	}
}
#[fixture]
fn task() -> Task {
	Task {
		id: id(1),
		workspace_id: id(2),
		title: "title 東京".into(),
		description: "description".into(),
		status: TaskStatus::Open,
		requirements: json!({}),
		owner: None,
		created_by: "operator".into(),
		dependencies: vec![],
		parent_id: None,
		revision: 7,
		created_at: Utc::now(),
	}
}
#[fixture]
fn inputs() -> Vec<(InputRead, String)> {
	vec![
		(
			InputRead {
				id: id(3),
				sequence: 4,
				digest: "first".into(),
			},
			"line one\nline two".into(),
		),
		(
			InputRead {
				id: id(3),
				sequence: 4,
				digest: "first".into(),
			},
			String::new(),
		),
		(
			InputRead {
				id: id(5),
				sequence: 8,
				digest: "second".into(),
			},
			"last".into(),
		),
	]
}
#[fixture]
fn repository() -> Repository {
	let matches = [(id(6), 2), (id(7), 3), (id(6), 2)]
		.map(|(entry_id, revision)| Match {
			entry_id,
			revision,
			source: json!({"kind":"message"}),
			agent: None,
			metadata: json!({"origin":"saved"}),
			text: "retrieved 東京".into(),
			score: 0.5,
		})
		.to_vec();
	Repository(Arc::new(Mutex::new(State {
		remote: false,
		local: Some(SearchResult {
			workspace_id: id(2),
			index_revision: 9,
			model: "embedding".into(),
			model_version: "1".into(),
			matches,
			estimated_tokens: 19,
			truncated: false,
		}),
		remote_result: Some(json!({"home":"context"})),
		calls: vec![],
		records: vec![],
		query: None,
		budget: None,
		fail: None,
		pending: None,
	})))
}
#[async_trait]
impl RunSemanticRepository for Repository {
	fn remote(&self) -> bool {
		self.0.lock().unwrap().remote
	}
	async fn suspend(&self) -> Result<()> {
		self.0.lock().unwrap().record("suspend")
	}
	async fn remote_context(
		&self,
		task: &Task,
		inputs: &[(InputRead, String)],
		query: &str,
		budget: usize,
	) -> Result<Option<Value>> {
		assert_eq!(task.id, id(1));
		assert_eq!(inputs.len(), 3);
		assert_eq!(inputs[0].0.digest, "first");
		assert_eq!(inputs[1].0.id, inputs[0].0.id);
		assert_eq!(inputs[2].0.sequence, 8);
		let mut state = self.0.lock().unwrap();
		state.record("remote")?;
		state.query = Some(query.into());
		state.budget = Some(budget);
		Ok(state.remote_result.clone())
	}
	async fn refresh_remote(&self) -> Result<()> {
		self.0.lock().unwrap().record("refresh")
	}
	async fn local_scope(&self) -> Result<Box<dyn RunSemanticScope + '_>> {
		self.0.lock().unwrap().record("local")?;
		Ok(Box::new(Scope(self.0.clone())))
	}
}
#[async_trait]
impl RunSemanticScope for Scope {
	async fn retrieve(&mut self, query: &str, budget: usize) -> Result<Option<SearchResult>> {
		let pending = {
			let mut state = self.0.lock().unwrap();
			state.record("retrieve")?;
			state.query = Some(query.into());
			state.budget = Some(budget);
			state.pending == Some("retrieve")
		};
		if pending {
			std::future::pending::<()>().await;
		}
		Ok(self.0.lock().unwrap().local.clone())
	}
	async fn begin_dependencies(&mut self) -> Result<Box<dyn RunSemanticJournal + '_>> {
		self.0.lock().unwrap().record("journal_begin")?;
		Ok(Box::new(Journal {
			state: self.0.clone(),
			committed: false,
		}))
	}
}
#[async_trait]
impl RunSemanticJournal for Journal {
	async fn record(&mut self, entry: Uuid, revision: i64) -> Result<()> {
		let pending = {
			let mut state = self.state.lock().unwrap();
			state.record("entry")?;
			state.records.push((entry, revision));
			state.pending == Some("entry")
		};
		if pending {
			std::future::pending::<()>().await;
		}
		Ok(())
	}
	async fn commit(mut self: Box<Self>) -> Result<()> {
		self.state.lock().unwrap().record("journal_commit")?;
		self.committed = true;
		Ok(())
	}
}
#[rstest]
#[tokio::test]
async fn local_context_commits_every_dependency_before_releasing_source_authority(
	repository: Repository,
	task: Task,
	inputs: Vec<(InputRead, String)>,
) {
	let expected =
		serde_json::to_value(repository.0.lock().unwrap().local.as_ref().unwrap()).unwrap();
	let value = retrieve(&repository, &task, &inputs, 123)
		.await
		.unwrap()
		.unwrap();
	assert_eq!(value, expected);
	let state = repository.0.lock().unwrap();
	assert_eq!(state.records, vec![(id(6), 2), (id(7), 3), (id(6), 2)]);
	assert_eq!(
		state.query.as_deref(),
		Some("title 東京\ndescription\nline one\nline two\n\nlast")
	);
	assert_eq!(state.budget, Some(123));
	assert_eq!(
		state.calls,
		vec![
			"local",
			"retrieve",
			"journal_begin",
			"entry",
			"entry",
			"entry",
			"journal_commit",
			"source_release"
		]
	);
}
#[rstest]
#[case::absent(true)]
#[case::empty_matches(false)]
#[tokio::test]
async fn absence_skips_the_journal_but_an_empty_result_still_commits_it(
	repository: Repository,
	task: Task,
	inputs: Vec<(InputRead, String)>,
	#[case] absent: bool,
) {
	{
		let mut state = repository.0.lock().unwrap();
		if absent {
			state.local = None;
		} else {
			state.local.as_mut().unwrap().matches.clear();
		}
	}
	let result = retrieve(&repository, &task, &inputs, 0).await.unwrap();
	assert_eq!(result.is_none(), absent);
	let state = repository.0.lock().unwrap();
	assert!(state.records.is_empty());
	assert_eq!(
		state.calls,
		if absent {
			vec!["local", "retrieve", "source_release"]
		} else {
			vec![
				"local",
				"retrieve",
				"journal_begin",
				"journal_commit",
				"source_release",
			]
		}
	);
}
#[rstest]
#[case::present(true)]
#[case::absent(false)]
#[tokio::test]
async fn a_remote_context_refreshes_admission_even_for_an_empty_home_response(
	repository: Repository,
	task: Task,
	inputs: Vec<(InputRead, String)>,
	#[case] present: bool,
) {
	{
		let mut state = repository.0.lock().unwrap();
		state.remote = true;
		if !present {
			state.remote_result = None;
		}
	}
	let expected = repository.0.lock().unwrap().remote_result.clone();
	assert_eq!(
		retrieve(&repository, &task, &inputs, 17).await.unwrap(),
		expected
	);
	let state = repository.0.lock().unwrap();
	assert_eq!(state.calls, vec!["suspend", "remote", "refresh"]);
	assert_eq!(
		state.query.as_deref(),
		Some("title 東京\ndescription\nline one\nline two\n\nlast")
	);
	assert_eq!(state.budget, Some(17));
	assert!(state.records.is_empty());
}
#[rstest]
#[tokio::test]
async fn an_empty_task_and_no_inputs_keep_the_original_single_newline_query(
	repository: Repository,
	mut task: Task,
) {
	task.title.clear();
	task.description.clear();
	retrieve(&repository, &task, &[], 1).await.unwrap();
	assert_eq!(repository.0.lock().unwrap().query.as_deref(), Some("\n"));
}
#[rstest]
#[case::suspend("suspend")]
#[case::home("remote")]
#[case::refresh("refresh")]
#[tokio::test]
async fn a_remote_boundary_fault_returns_no_context_and_never_falls_back_locally(
	repository: Repository,
	task: Task,
	inputs: Vec<(InputRead, String)>,
	#[case] boundary: &'static str,
) {
	{
		let mut state = repository.0.lock().unwrap();
		state.remote = true;
		state.fail = Some(boundary);
	}
	let Error::Port(error) = retrieve(&repository, &task, &inputs, 7)
		.await
		.err()
		.unwrap()
	else {
		panic!("expected boundary fault")
	};
	assert_eq!(
		error.downcast_ref::<std::io::Error>().unwrap().to_string(),
		format!("{boundary} fault")
	);
	let expected = ["suspend", "remote", "refresh"];
	let last = expected.iter().position(|s| *s == boundary).unwrap();
	assert_eq!(repository.0.lock().unwrap().calls, expected[..=last]);
}
#[rstest]
#[case::scope("local", false, false)]
#[case::retrieval("retrieve", true, false)]
#[case::journal("journal_begin", true, false)]
#[case::dependency("entry", true, true)]
#[case::commit("journal_commit", true, true)]
#[tokio::test]
async fn failed_local_journaling_cannot_disclose_retrieved_text(
	repository: Repository,
	task: Task,
	inputs: Vec<(InputRead, String)>,
	#[case] boundary: &'static str,
	#[case] scope_exists: bool,
	#[case] journal_exists: bool,
) {
	repository.0.lock().unwrap().fail = Some(boundary);
	let Error::Port(error) = retrieve(&repository, &task, &inputs, 7)
		.await
		.err()
		.unwrap()
	else {
		panic!("expected journal fault")
	};
	assert_eq!(
		error.downcast_ref::<std::io::Error>().unwrap().to_string(),
		format!("{boundary} fault")
	);
	let state = repository.0.lock().unwrap();
	assert_eq!(state.calls.contains(&"source_release".into()), scope_exists);
	assert_eq!(
		state.calls.contains(&"journal_rollback".into()),
		journal_exists
	);
	assert!(!state.calls.contains(&"remote".into()));
	if journal_exists {
		assert_eq!(
			&state.calls[state.calls.len() - 2..],
			["journal_rollback", "source_release"]
		);
	}
}
#[rstest]
#[case::retrieval("retrieve", false)]
#[case::journal_entry("entry", true)]
#[tokio::test]
async fn cancelled_context_acquisition_releases_the_retained_source_and_provisional_journal(
	repository: Repository,
	task: Task,
	inputs: Vec<(InputRead, String)>,
	#[case] boundary: &'static str,
	#[case] journal_exists: bool,
) {
	repository.0.lock().unwrap().pending = Some(boundary);
	assert!(
		tokio::time::timeout(
			std::time::Duration::from_millis(10),
			retrieve(&repository, &task, &inputs, 7)
		)
		.await
		.is_err()
	);
	let state = repository.0.lock().unwrap();
	assert_eq!(
		state.calls.last().map(String::as_str),
		Some("source_release")
	);
	assert_eq!(
		state.calls.contains(&"journal_rollback".into()),
		journal_exists
	);
	assert!(!state.calls.contains(&"journal_commit".into()));
}
