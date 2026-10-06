use super::*;
use aidash_domain::Workspace;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rstest::rstest;
use uuid::Uuid;
fn now() -> DateTime<Utc> {
	DateTime::from_timestamp(1_000_000, 0).unwrap()
}
fn options() -> GraphOptions {
	GraphOptions {
		scope_workspace: None,
		depth: 1,
		mode: "mesh".into(),
		kinds: vec!["workspace".into()],
		relations: vec![],
		hours: 0,
		limit: 2,
		cursor: None,
		target_tenant: None,
	}
}
fn viewer() -> GraphViewer {
	GraphViewer::Subject {
		tenant: "tenant".into(),
		subject: "root".into(),
	}
}
fn workspace(n: u128) -> Candidate {
	Candidate::Workspace(Workspace {
		id: Uuid::from_u128(n),
		title: format!("workspace {n}"),
		goal: String::new(),
		state: json!({}),
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
}
impl Scope {
	fn new(n: u128) -> Self {
		Self {
			rows: (1..=n).map(workspace).collect(),
			deny_through: 0,
			calls: vec![],
			scoped: vec![],
			generations: 0,
			change_generation: false,
			fail: false,
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
		Ok(match c {
			Candidate::Workspace(w) => w.id.as_u128() > self.deny_through,
			_ => false,
		})
	}
	async fn linked_workspace(&mut self, _: Uuid) -> Result<Option<Candidate>> {
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
		_: &[GraphNode],
		_: i64,
	) -> Result<Vec<GraphActivity>> {
		Ok(vec![])
	}
}
#[tokio::test]
async fn hidden_first_page_does_not_end_the_visible_scan() {
	let mut s = Scope::new(72);
	s.deny_through = 70;
	let p = project(&mut s, "aidash://source", &viewer(), &options())
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
	let first = project(&mut s, "aidash://source", &viewer(), &o)
		.await
		.unwrap();
	let cursor = s
		.decode_cursor(first.next_cursor.as_deref().unwrap())
		.unwrap();
	assert_eq!(cursor.offset, 2);
	assert_eq!(cursor.kind, 0);
	o.cursor = first.next_cursor;
	let second = project(&mut s, "aidash://source", &viewer(), &o)
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
	let error = project(&mut s, "aidash://source", &viewer(), &options())
		.await
		.unwrap_err();
	assert!(matches!(error,Error::Conflict(ref text) if text=="graph projection changed"));
}
#[tokio::test]
async fn adapter_failure_keeps_its_identity() {
	let mut s = Scope::new(1);
	s.fail = true;
	let error = project(&mut s, "aidash://source", &viewer(), &options())
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
	o.scope_workspace = Some(Uuid::from_u128(8));
	project(&mut s, "aidash://source", &viewer(), &o)
		.await
		.unwrap();
	assert_eq!(s.scoped, vec![true]);
}
#[tokio::test]
async fn an_oversized_first_group_is_rejected_without_advancing_a_cursor() {
	let mut s = Scope::new(1);
	let Candidate::Workspace(w) = &mut s.rows[0] else {
		panic!("workspace fixture")
	};
	w.title = "x".repeat(3_000_001);
	let error = project(&mut s, "aidash://source", &viewer(), &options())
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
	let first = cursor_binding("aidash://source", &viewer(), &o).unwrap();
	o.cursor = Some("opaque".into());
	assert_eq!(
		cursor_binding("aidash://source", &viewer(), &o).unwrap(),
		first
	);
	o.limit += 1;
	assert_ne!(
		cursor_binding("aidash://source", &viewer(), &o).unwrap(),
		first
	);
}
