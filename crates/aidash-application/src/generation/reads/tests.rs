use super::*;
use crate::{
	generation::test_support,
	ports::generation::{
		reads::{GenerationPages, GenerationReadScope},
		visibility::GenerationVisibility,
	},
};
use aidash_domain::{entities::Task, policy::Resource};
use async_trait::async_trait;
use rstest::rstest;
use serde_json::Value;
use std::{
	collections::BTreeSet,
	sync::{Arc, Mutex},
	time::Duration,
};

struct State {
	rows: Vec<Request>,
	job: Option<Request>,
	denied: BTreeSet<Uuid>,
	document: Value,
	calls: Vec<String>,
	active: usize,
	commits: usize,
	rollbacks: usize,
	failure: Option<&'static str>,
	pause_page: bool,
}
struct Repository {
	principal: Principal,
	state: Arc<Mutex<State>>,
}
impl Repository {
	fn new(subject: bool) -> Self {
		let mut job = test_support::request(1);
		job.home_node = "aidash://home".into();
		Self {
			principal: if subject {
				Principal::Subject {
					tenant: "tenant".into(),
					subject: "alice".into(),
				}
			} else {
				Principal::Operator
			},
			state: Arc::new(Mutex::new(State {
				rows: vec![],
				job: Some(job),
				denied: BTreeSet::new(),
				document: test_support::specification(),
				calls: vec![],
				active: 0,
				commits: 0,
				rollbacks: 0,
				failure: None,
				pause_page: false,
			})),
		}
	}
	fn seed(&self, count: usize) {
		let mut state = self.state.lock().unwrap();
		let base = state.job.as_ref().unwrap().clone();
		state.rows = (1..=count)
			.map(|i| {
				let mut row = base.clone();
				row.id = Uuid::from_u128(i as u128);
				row
			})
			.collect();
	}
	fn calls(&self) -> Vec<String> {
		self.state.lock().unwrap().calls.clone()
	}
}
struct Pages {
	state: Arc<Mutex<State>>,
	subject: bool,
}
impl Drop for Pages {
	fn drop(&mut self) {
		self.state.lock().unwrap().calls.push("pages.drop".into());
	}
}
#[async_trait]
impl GenerationPages for Pages {
	async fn page(&mut self, _tenant: &str, offset: usize) -> Result<Vec<Request>> {
		let pause = {
			let mut state = self.state.lock().unwrap();
			if self.subject {
				assert_eq!(state.active, 1, "authority must cover every page");
			}
			state.calls.push(format!("page:{offset}"));
			if state.failure == Some("page") {
				return Err(Error::External("page failed".into()));
			}
			state.pause_page
		};
		if pause {
			std::future::pending::<()>().await;
		}
		Ok(self
			.state
			.lock()
			.unwrap()
			.rows
			.iter()
			.skip(offset)
			.take(200)
			.cloned()
			.collect())
	}
}
struct Scope {
	state: Arc<Mutex<State>>,
	completed: bool,
}
impl Scope {
	fn finish<T>(&mut self, result: Result<T>) -> Result<T> {
		let mut state = self.state.lock().unwrap();
		state.calls.push("finish".into());
		state.active -= 1;
		self.completed = true;
		if result.is_ok() {
			state.commits += 1;
		} else {
			state.rollbacks += 1;
		}
		result
	}
	fn point(&self, operation: &'static str) -> Result<()> {
		let mut state = self.state.lock().unwrap();
		assert_eq!(state.active, 1);
		state.calls.push(operation.into());
		if state.failure == Some(operation) {
			return Err(Error::External(format!("{operation} failed")));
		}
		Ok(())
	}
}
impl Drop for Scope {
	fn drop(&mut self) {
		if !self.completed {
			let mut state = self.state.lock().unwrap();
			state.active -= 1;
			state.rollbacks += 1;
			state.calls.push("scope.drop".into());
		}
	}
}
fn usage_value() -> Usage {
	Usage {
		token_limit: 10000,
		used_tokens: 2000,
		inference_attempts: 3,
		compaction_call_limit: 10,
		compaction_calls: 2,
		embedding_calls: 4,
		embedding_call_limit: 20,
	}
}
#[async_trait]
impl GenerationReads for Repository {
	fn principal(&self) -> &Principal {
		&self.principal
	}
	async fn pages(&self) -> Result<Box<dyn GenerationPages>> {
		let mut state = self.state.lock().unwrap();
		state.calls.push("pages".into());
		if state.failure == Some("pages") {
			return Err(Error::External("pages failed".into()));
		}
		Ok(Box::new(Pages {
			state: self.state.clone(),
			subject: matches!(self.principal, Principal::Subject { .. }),
		}))
	}
	async fn begin_subject(&self) -> Result<Box<dyn GenerationReadScope>> {
		let mut state = self.state.lock().unwrap();
		state.calls.push("begin".into());
		state.active += 1;
		Ok(Box::new(Scope {
			state: self.state.clone(),
			completed: false,
		}))
	}
	async fn operator_history(&self, _tenant: &str, _id: Uuid) -> Result<Vec<History>> {
		self.state
			.lock()
			.unwrap()
			.calls
			.push("operator.history".into());
		Ok(vec![])
	}
	async fn operator_usage(&self, _tenant: &str, _id: Uuid) -> Result<Usage> {
		self.state
			.lock()
			.unwrap()
			.calls
			.push("operator.usage".into());
		Ok(usage_value())
	}
	async fn operator_specification(&self, _tenant: &str, _id: Uuid) -> Result<Value> {
		let mut state = self.state.lock().unwrap();
		state.calls.push("operator.specification".into());
		Ok(state.document.clone())
	}
}
#[async_trait]
impl GenerationVisibility for Scope {
	fn inherited_lease(&self) -> bool {
		false
	}
	fn context(&mut self, _value: Value) {}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		Resource {
			tenant: "tenant".into(),
			kind: kind.into(),
			id: id.into(),
			attributes,
		}
	}
	async fn workspace(&mut self, _id: Uuid) -> Result<Resource> {
		panic!("foreign request must not query a local workspace")
	}
	async fn task(&mut self, _id: Uuid, _workspace: Uuid) -> Result<Option<Task>> {
		panic!("foreign request must not query a local task")
	}
	async fn task_visible(&mut self, _task: &Task) -> Result<bool> {
		panic!("foreign request has a qualified remote task")
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		let mut state = self.state.lock().unwrap();
		assert_eq!(state.active, 1);
		state.calls.push(format!("decide:{action}:{}", resource.id));
		Ok(action != "generation.read"
			|| !state
				.denied
				.contains(&Uuid::parse_str(&resource.id).unwrap()))
	}
}
#[async_trait]
impl GenerationReadScope for Scope {
	async fn job(&mut self, _tenant: &str, _id: Uuid) -> Result<Option<Request>> {
		self.point("job")?;
		Ok(self.state.lock().unwrap().job.clone())
	}
	async fn history(&mut self, _tenant: &str, _id: Uuid) -> Result<Vec<History>> {
		self.point("history")?;
		Ok(vec![])
	}
	async fn usage(&mut self, _tenant: &str, _id: Uuid) -> Result<Usage> {
		self.point("usage")?;
		Ok(usage_value())
	}
	async fn specification(&mut self, _tenant: &str, _id: Uuid) -> Result<Value> {
		self.point("specification")?;
		Ok(self.state.lock().unwrap().document.clone())
	}
	async fn finish_requests(
		mut self: Box<Self>,
		result: Result<Vec<Request>>,
	) -> Result<Vec<Request>> {
		self.finish(result)
	}
	async fn finish_history(
		mut self: Box<Self>,
		result: Result<Vec<History>>,
	) -> Result<Vec<History>> {
		self.finish(result)
	}
	async fn finish_usage(mut self: Box<Self>, result: Result<Usage>) -> Result<Usage> {
		self.finish(result)
	}
	async fn finish_specification(mut self: Box<Self>, result: Result<Spec>) -> Result<Spec> {
		self.finish(result)
	}
}

#[rstest]
#[tokio::test]
async fn subject_scan_skips_denied_rows_and_stops_after_200_visible_requests_under_one_lease() {
	let repo = Repository::new(true);
	repo.seed(550);
	repo.state.lock().unwrap().denied = (1..=300).map(Uuid::from_u128).collect();
	let rows = list(&repo, "tenant").await.unwrap();
	assert_eq!(
		rows.iter().map(|r| r.id).collect::<Vec<_>>(),
		(301..=500).map(Uuid::from_u128).collect::<Vec<_>>()
	);
	assert_eq!(
		repo.calls()
			.into_iter()
			.filter(|c| c.starts_with("page:"))
			.collect::<Vec<_>>(),
		vec!["page:0", "page:200", "page:400"]
	);
	let calls = repo.calls();
	assert_eq!(&calls[..2], ["pages", "begin"]);
	assert_eq!(&calls[calls.len() - 2..], ["finish", "pages.drop"]);
	let state = repo.state.lock().unwrap();
	assert_eq!((state.active, state.commits, state.rollbacks), (0, 1, 0));
}
#[rstest]
#[tokio::test]
async fn operator_keeps_one_unfiltered_page_and_does_not_open_subject_authority() {
	let repo = Repository::new(false);
	repo.seed(550);
	repo.state.lock().unwrap().denied = (1..=550).map(Uuid::from_u128).collect();
	let rows = list(&repo, "tenant").await.unwrap();
	assert_eq!(rows.len(), 200);
	assert_eq!(rows[0].id, Uuid::from_u128(1));
	assert_eq!(rows[199].id, Uuid::from_u128(200));
	assert_eq!(repo.calls(), vec!["pages", "page:0", "pages.drop"]);
}
#[rstest]
#[tokio::test]
async fn all_denied_pages_exhaust_without_repeating_or_skipping_a_cursor() {
	let repo = Repository::new(true);
	repo.seed(400);
	repo.state.lock().unwrap().denied = (1..=400).map(Uuid::from_u128).collect();
	assert!(list(&repo, "tenant").await.unwrap().is_empty());
	assert_eq!(
		repo.calls()
			.into_iter()
			.filter(|c| c.starts_with("page:"))
			.collect::<Vec<_>>(),
		vec!["page:0", "page:200", "page:400"]
	);
}
#[rstest]
#[tokio::test]
async fn cross_tenant_list_acquires_the_existing_page_owner_without_querying_rows_or_authority() {
	let repo = Repository::new(true);
	assert!(matches!(list(&repo, "other").await, Err(Error::Forbidden)));
	assert_eq!(repo.calls(), vec!["pages", "pages.drop"]);
}
#[rstest]
#[tokio::test]
async fn page_owner_failure_preserves_the_original_error_before_tenant_dispatch() {
	let repo = Repository::new(true);
	repo.state.lock().unwrap().failure = Some("pages");
	assert!(
		matches!(list(&repo,"other").await,Err(Error::External(message)) if message=="pages failed")
	);
	assert_eq!(repo.calls(), vec!["pages"]);
}
#[rstest]
#[tokio::test]
async fn page_failure_finishes_and_rolls_back_the_same_subject_scope() {
	let repo = Repository::new(true);
	repo.state.lock().unwrap().failure = Some("page");
	assert!(
		matches!(list(&repo,"tenant").await,Err(Error::External(message)) if message=="page failed")
	);
	assert_eq!(
		repo.calls(),
		vec!["pages", "begin", "page:0", "finish", "pages.drop"]
	);
	let state = repo.state.lock().unwrap();
	assert_eq!((state.active, state.commits, state.rollbacks), (0, 0, 1));
}
#[rstest]
#[tokio::test]
async fn cancellation_releases_page_owner_and_subject_scope_without_success_audit() {
	let repo = Repository::new(true);
	repo.state.lock().unwrap().pause_page = true;
	assert!(
		tokio::time::timeout(Duration::from_millis(10), list(&repo, "tenant"))
			.await
			.is_err()
	);
	let state = repo.state.lock().unwrap();
	assert_eq!((state.active, state.commits, state.rollbacks), (0, 0, 1));
	assert_eq!(
		&state.calls[state.calls.len() - 2..],
		["scope.drop", "pages.drop"]
	);
}
async fn detail(repo: &Repository, tenant: &str, operation: &str) -> Result<()> {
	match operation {
		"history" => history(repo, tenant, Uuid::from_u128(1)).await.map(|_| ()),
		"usage" => usage(repo, tenant, Uuid::from_u128(1)).await.map(|_| ()),
		"specification" => specification(repo, tenant, Uuid::from_u128(1))
			.await
			.map(|_| ()),
		_ => panic!("unknown operation"),
	}
}
#[rstest]
#[case("history")]
#[case("usage")]
#[case("specification")]
#[tokio::test]
async fn direct_subject_detail_requires_current_visibility_before_any_ledger_read(
	#[case] operation: &str,
) {
	let repo = Repository::new(true);
	repo.state.lock().unwrap().denied.insert(Uuid::from_u128(1));
	assert!(matches!(
		detail(&repo, "tenant", operation).await,
		Err(Error::Forbidden)
	));
	assert!(
		!repo
			.calls()
			.iter()
			.any(|c| c == operation || c.starts_with("operator."))
	);
	assert_eq!(repo.calls().last().unwrap(), "finish");
	assert_eq!(repo.state.lock().unwrap().rollbacks, 1);
}
#[rstest]
#[case("history")]
#[case("usage")]
#[case("specification")]
#[tokio::test]
async fn missing_subject_request_returns_forbidden_and_never_reads_detail(#[case] operation: &str) {
	let repo = Repository::new(true);
	repo.state.lock().unwrap().job = None;
	assert!(matches!(
		detail(&repo, "tenant", operation).await,
		Err(Error::Forbidden)
	));
	assert_eq!(repo.calls(), vec!["begin", "job", "finish"]);
}
#[rstest]
#[case("history")]
#[case("usage")]
#[case("specification")]
#[tokio::test]
async fn cross_tenant_detail_rejects_before_authority_or_operator_query(#[case] operation: &str) {
	let repo = Repository::new(true);
	assert!(matches!(
		detail(&repo, "other", operation).await,
		Err(Error::Forbidden)
	));
	assert!(repo.calls().is_empty());
}
#[rstest]
#[case::subject(true)]
#[case::operator(false)]
#[tokio::test]
async fn usage_and_pinned_specification_preserve_the_saved_contract(#[case] subject: bool) {
	let repo = Repository::new(subject);
	let usage = usage(&repo, "tenant", Uuid::from_u128(1)).await.unwrap();
	assert_eq!(
		serde_json::to_value(usage).unwrap(),
		serde_json::to_value(usage_value()).unwrap()
	);
	let spec = specification(&repo, "tenant", Uuid::from_u128(1))
		.await
		.unwrap();
	assert_eq!(
		serde_json::to_value(spec).unwrap(),
		test_support::specification()
	);
	if subject {
		assert_eq!(repo.state.lock().unwrap().commits, 2);
	} else {
		assert!(!repo.calls().contains(&"begin".into()));
	}
}
#[rstest]
#[case::subject(true)]
#[case::operator(false)]
#[tokio::test]
async fn malformed_pinned_specification_retains_json_failure(#[case] subject: bool) {
	let repo = Repository::new(subject);
	repo.state.lock().unwrap().document = serde_json::json!({"corrupt":true});
	assert!(matches!(
		specification(&repo, "tenant", Uuid::from_u128(1)).await,
		Err(Error::Json(_))
	));
	if subject {
		assert_eq!(repo.state.lock().unwrap().rollbacks, 1);
	}
}
