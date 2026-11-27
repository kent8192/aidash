use super::*;
use aidash_domain::Thread;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rstest::rstest;
use uuid::Uuid;
fn now() -> DateTime<Utc> {
	DateTime::from_timestamp(1_000_000, 0).unwrap()
}
fn options() -> GraphOptions {
	GraphOptions {
		scope_thread: None,
		depth: 1,
		mode: "mesh".into(),
		kinds: vec!["thread".into()],
		relations: vec![],
		hours: 0,
		limit: 2,
		cursor: None,
	}
}
fn subject() -> &'static str {
	"root"
}
fn thread(n: u128) -> Candidate {
	Candidate::Thread(Thread {
		scratch_space_id: uuid::Uuid::nil(),
		node: "brave-otter-k3m9q2xa".parse().unwrap(),
		id: Uuid::from_u128(n),
		title: format!("thread {n}"),
		created_by: "subject".into(),
		revision: 0,
		created_at: now(),
	})
}
struct Scope {
	rows: Vec<Candidate>,
	deny_through: u128,
	calls: Vec<(u8, u64)>,
	scoped: Vec<bool>,
	generations: usize,
	change_generation: bool,
	fail: bool,
	silent_row: Option<u128>,
	row_silence: Option<String>,
	activity_positions: Vec<(Vec<String>, Option<i64>)>,
}
impl Scope {
	fn new(n: u128) -> Self {
		Self {
			rows: (1..=n).map(thread).collect(),
			deny_through: 0,
			calls: vec![],
			scoped: vec![],
			generations: 0,
			change_generation: false,
			fail: false,
			silent_row: None,
			row_silence: None,
			activity_positions: vec![],
		}
	}
}
#[async_trait]
impl GraphProjectionScope for Scope {
	fn node_id(&self) -> &str {
		"aidash://b"
	}
	fn now(&self) -> DateTime<Utc> {
		now()
	}
	fn encode_cursor(&self, c: &GraphCursor) -> Result<String> {
		Ok(serde_json::to_string(c)?)
	}
	fn decode_cursor(&self, t: &str) -> Result<GraphCursor> {
		Ok(serde_json::from_str(t)?)
	}
	async fn generation(&mut self, _: &GraphOptions) -> Result<String> {
		self.generations += 1;
		Ok(if self.change_generation && self.generations > 1 {
			"changed"
		} else {
			"stable"
		}
		.into())
	}
	async fn candidates(&mut self, k: u8, offset: u64, _: &GraphOptions) -> Result<Vec<Candidate>> {
		if self.fail {
			return Err(Error::Port(Box::new(std::io::Error::other(
				"snapshot adapter fault",
			))));
		}
		self.calls.push((k, offset));
		Ok(if k == 0 {
			self.rows
				.iter()
				.skip(offset as usize)
				.take(64)
				.cloned()
				.collect()
		} else {
			vec![]
		})
	}
	async fn visible(&mut self, c: &Candidate, scoped: bool) -> Result<bool> {
		self.scoped.push(scoped);
		if let Candidate::Task(w) = c
			&& self.silent_row == Some(w.id.as_u128())
		{
			self.row_silence = Some("aidash://silent-c".into());
			return Ok(false);
		}
		Ok(match c {
			Candidate::Thread(w) => w.id.as_u128() > self.deny_through,
			Candidate::Task(w) => w.id.as_u128() > self.deny_through,
			_ => false,
		})
	}
	async fn linked_thread(&mut self, _: Uuid) -> Result<Option<Candidate>> {
		Ok(None)
	}
	async fn linked_task(&mut self, _: Uuid) -> Result<Option<Candidate>> {
		Ok(None)
	}
	async fn linked_registry(&mut self, _: &str, _: &str, _: &str) -> Result<Option<Candidate>> {
		Ok(None)
	}
	async fn activity(
		&mut self,
		_: &GraphOptions,
		nodes: &[GraphNode],
		_: i64,
		before: Option<i64>,
	) -> Result<(Vec<GraphActivity>, Option<i64>)> {
		self.activity_positions
			.push((nodes.iter().map(|node| node.id.clone()).collect(), before));
		Ok((vec![], before.is_none().then_some(100)))
	}
}

#[tokio::test]
async fn activity_continues_with_the_same_nodes_and_an_independent_cursor() {
	let mut scope = Scope::new(4);
	let mut options = options();
	let first = project(&mut scope, "aidash://source", subject(), &options)
		.await
		.unwrap();
	assert!(first.activity_more);
	options.cursor = first.activity_cursor;
	let second = project(&mut scope, "aidash://source", subject(), &options)
		.await
		.unwrap();
	assert_eq!(second.next_cursor, first.next_cursor);
	assert!(!second.activity_more);
	assert_eq!(second.activity_cursor, None);
	assert_eq!(scope.activity_positions[0].0, scope.activity_positions[1].0);
	assert_eq!(scope.activity_positions[0].1, None);
	assert_eq!(scope.activity_positions[1].1, Some(100));
}

#[tokio::test]
async fn hidden_first_page_does_not_end_the_visible_scan() {
	let mut s = Scope::new(72);
	s.deny_through = 70;
	let p = project(&mut s, "aidash://source", subject(), &options())
		.await
		.unwrap();
	assert_eq!(p.nodes.len(), 2);
	assert_eq!(
		p.nodes[0].resource_id,
		Some(Uuid::from_u128(71).to_string())
	);
	assert!(s.calls.contains(&(0, 64)));
	assert_eq!(s.generations, 2);
}
#[tokio::test]
async fn continuation_retries_the_first_record_that_did_not_fit() {
	let mut s = Scope::new(5);
	let mut o = options();
	let first = project(&mut s, "aidash://source", subject(), &o)
		.await
		.unwrap();
	let cursor = s
		.decode_cursor(first.next_cursor.as_deref().unwrap())
		.unwrap();
	assert_eq!(cursor.offset, 2);
	assert_eq!(cursor.kind, 0);
	o.cursor = first.next_cursor;
	let second = project(&mut s, "aidash://source", subject(), &o)
		.await
		.unwrap();
	assert_eq!(
		second.nodes[0].resource_id,
		Some(Uuid::from_u128(3).to_string())
	);
	assert!(
		first
			.nodes
			.iter()
			.all(|one| second.nodes.iter().all(|two| one.id != two.id))
	);
}
#[tokio::test]
async fn generation_change_refuses_a_mixed_snapshot() {
	let mut s = Scope::new(1);
	s.change_generation = true;
	let error = project(&mut s, "aidash://source", subject(), &options())
		.await
		.unwrap_err();
	assert!(matches!(error,Error::Conflict(ref text) if text=="graph projection changed"));
}
#[tokio::test]
async fn adapter_failure_keeps_its_identity() {
	let mut s = Scope::new(1);
	s.fail = true;
	let error = project(&mut s, "aidash://source", subject(), &options())
		.await
		.unwrap_err();
	assert!(
		matches!(error,Error::Port(ref source) if source.to_string()=="snapshot adapter fault")
	);
}
#[tokio::test]
async fn scoped_projection_uses_the_receiving_admission_authority() {
	let mut s = Scope::new(1);
	let mut o = options();
	o.scope_thread = Some(Uuid::from_u128(8));
	project(&mut s, "aidash://source", subject(), &o)
		.await
		.unwrap();
	assert_eq!(s.scoped, vec![true]);
}
#[tokio::test]
async fn an_oversized_first_group_is_rejected_without_advancing_a_cursor() {
	let mut s = Scope::new(1);
	let Candidate::Thread(w) = &mut s.rows[0] else {
		panic!("thread fixture")
	};
	w.title = "x".repeat(3_000_001);
	let error = project(&mut s, "aidash://source", subject(), &options())
		.await
		.unwrap_err();
	assert!(
		matches!(error,Error::Invalid(ref text) if text=="graph resource exceeds response limit")
	);
	assert_eq!(s.generations, 1);
}
fn cursor() -> GraphCursor {
	GraphCursor {
		kind: 0,
		offset: 0,
		activity_before: None,
		generation: "stable".into(),
		binding: "scope".into(),
		window_end: 1_000_000,
		expires_at: 1_000_300,
	}
}
#[rstest]
#[case("binding")]
#[case("kind")]
#[case("future")]
#[case("duration")]
fn invalid_cursor_is_forbidden(#[case] field: &str) {
	let mut c = cursor();
	match field {
		"binding" => c.binding = "other".into(),
		"kind" => c.kind = 6,
		"future" => {
			c.window_end += 61;
			c.expires_at += 61
		}
		"duration" => c.expires_at += 1,
		_ => panic!("invalid case"),
	};
	assert!(matches!(
		checked_cursor(c, "scope", "stable", 1_000_000),
		Err(Error::Forbidden)
	));
}
#[rstest]
#[case("expiry")]
#[case("generation")]
fn stale_cursor_requires_restart(#[case] field: &str) {
	let mut c = cursor();
	if field == "expiry" {
		c.window_end -= 300;
		c.expires_at -= 300;
	} else {
		c.generation = "previous".into();
	}
	assert!(
		matches!(checked_cursor(c,"scope","stable",1_000_000),Err(Error::Conflict(ref text)) if text=="graph projection changed")
	);
}
#[test]
fn continuation_is_not_part_of_the_scope_digest() {
	let mut o = options();
	let first = cursor_binding("aidash://source", subject(), &o).unwrap();
	o.cursor = Some("opaque".into());
	assert_eq!(
		cursor_binding("aidash://source", subject(), &o).unwrap(),
		first
	);
	o.limit += 1;
	assert_ne!(
		cursor_binding("aidash://source", subject(), &o).unwrap(),
		first
	);
}

impl crate::ports::authorization::visibility::RowOutcome for Scope {
	fn begin_row(&mut self) {
		self.row_silence = None;
	}
	fn take_row_silence(&mut self) -> Option<String> {
		self.row_silence.take()
	}
}

#[tokio::test]
async fn graph_reports_unconfirmed_rows_under_the_answering_node() {
	let mut scope = Scope::new(0);
	scope.rows = (1..=3)
		.map(|n| {
			Candidate::Task(aidash_domain::Task {
				id: Uuid::from_u128(n),
				thread_id: Uuid::from_u128(10),
				title: "task".into(),
				description: String::new(),
				status: aidash_domain::TaskStatus::Open,
				requirements: json!({}),
				owner: None,
				created_by: "user".into(),
				dependencies: vec![],
				parent_id: None,
				started_by_duty: None,
				revision: 0,
				created_at: now(),
			})
		})
		.collect();
	let mut options = options();
	options.kinds = vec!["task".into()];
	scope.silent_row = Some(1);
	scope.deny_through = 2;
	let page = project(&mut scope, "aidash://source", subject(), &options)
		.await
		.unwrap();
	let body = serde_json::to_value(page).unwrap();
	assert_eq!(body["nodes"].as_array().unwrap().len(), 1);
	assert_eq!(
		body["unconfirmed"],
		json!([{ "node": "aidash://b", "kind": "task", "thread_id": Uuid::from_u128(10), "count": 1 }])
	);
	assert!(!body.to_string().contains("aidash://silent-c"));
	scope.silent_row = None;
	scope.deny_through = 0;
	options.limit = 4;
	let recovered = project(&mut scope, "aidash://source", subject(), &options)
		.await
		.unwrap();
	assert_eq!(recovered.nodes.len(), 3);
	assert!(recovered.unconfirmed.is_empty());
}
