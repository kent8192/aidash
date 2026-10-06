use super::*;
use crate::ports::authorization::stream::AuthorityRecord;
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use rstest::{fixture, rstest};
use serde_json::Value;
use std::{
	cell::Cell,
	sync::atomic::{AtomicUsize, Ordering},
};
fn now() -> DateTime<Utc> {
	DateTime::from_timestamp(1_790_000_000, 0).unwrap()
}
struct Authority {
	document: Value,
	present: bool,
	mapping: bool,
	mapping_enabled: Option<bool>,
	issuer: Option<String>,
	validated: Option<DateTime<Utc>>,
	disabled: Option<DateTime<Utc>>,
	owner: Option<String>,
	reads: AtomicUsize,
	fail: bool,
}
#[fixture]
fn authority_store() -> Authority {
	let authority = Authority {
		document: json!({"tenant":"tenant","subjects":{"reader":{"kind":"user"},"parent":{"kind":"user"}},"policies":[{"id":"stream","effect":"allow","subjects":{"any":true},"actions":["workspace.read","workspace.events"],"resources":{"kinds":["workspace"]},"condition":{"op":"all","conditions":[{"op":"eq","left":{"source":"resource","path":"/owner"},"right":{"source":"literal","value":"saved-owner"}},{"op":"eq","left":{"source":"environment","path":"/transport"},"right":{"source":"literal","value":"api"}},{"op":"eq","left":{"source":"environment","path":"/node_id"},"right":{"source":"literal","value":"aidash://local"}}]}}]}),
		present: true,
		mapping: false,
		mapping_enabled: Some(true),
		issuer: Some("external".into()),
		validated: Some(now()),
		disabled: None,
		owner: Some("saved-owner".into()),
		reads: AtomicUsize::new(0),
		fail: false,
	};
	let bundle: aidash_domain::policy::PolicyBundle =
		serde_json::from_value(authority.document.clone()).unwrap();
	bundle.validate().unwrap();
	authority
}
#[async_trait]
impl StreamAuthorityStore for Authority {
	fn tenant(&self) -> &str {
		"tenant"
	}
	fn subject(&self) -> &str {
		"reader"
	}
	fn node_id(&self) -> &str {
		"aidash://local"
	}
	fn google_issuer(&self) -> &str {
		"google"
	}
	fn now(&self) -> DateTime<Utc> {
		now()
	}
	async fn current(&self, _: Option<Uuid>) -> Result<Option<AuthorityRecord>> {
		self.reads.fetch_add(1, Ordering::SeqCst);
		if self.fail {
			return Err(Error::Port(Box::new(std::io::Error::other("snapshot"))));
		}
		Ok(self.present.then(|| AuthorityRecord {
			revision: 31,
			document: self.document.clone(),
			owner_subject: self.owner.clone(),
			mapping_id: self.mapping.then_some(Uuid::from_u128(9)),
			mapping_enabled: self.mapping_enabled,
			issuer: self.issuer.clone(),
			last_valid_at: self.validated,
			disabled_at: self.disabled,
		}))
	}
}
#[rstest]
#[case::global(None)]
#[case::workspace(Some(Uuid::from_u128(1)))]
#[tokio::test]
async fn idle_authority_uses_one_snapshot_and_the_saved_owner_environment(
	authority_store: Authority,
	#[case] workspace: Option<Uuid>,
) {
	authority(&authority_store, workspace).await.unwrap();
	assert_eq!(authority_store.reads.load(Ordering::SeqCst), 1);
}
#[rstest]
#[tokio::test]
async fn absent_current_credentials_remain_unauthorized(mut authority_store: Authority) {
	authority_store.present = false;
	assert!(matches!(
		authority(&authority_store, None).await,
		Err(Error::Unauthorized)
	));
	assert_eq!(authority_store.reads.load(Ordering::SeqCst), 1);
}
#[rstest]
#[case::missing(None)]
#[case::disabled(Some(false))]
#[tokio::test]
async fn disabled_dashboard_mapping_denies_before_policy_decoding(
	mut authority_store: Authority,
	#[case] enabled: Option<bool>,
) {
	authority_store.mapping = true;
	authority_store.mapping_enabled = enabled;
	authority_store.document = json!("invalid");
	assert!(matches!(
		authority(&authority_store, None).await,
		Err(Error::Forbidden)
	));
}
#[rstest]
#[case::issuer(0)]
#[case::validation(1)]
#[case::disabled(2)]
#[tokio::test]
async fn incomplete_dashboard_status_denies_idle_streams(
	mut authority_store: Authority,
	#[case] missing: u8,
) {
	authority_store.mapping = true;
	match missing {
		0 => authority_store.issuer = None,
		1 => authority_store.validated = None,
		_ => authority_store.disabled = Some(now()),
	};
	assert!(matches!(
		authority(&authority_store, None).await,
		Err(Error::Forbidden)
	));
}
#[rstest]
#[case::stale_external("external", false)]
#[case::google("google", true)]
#[tokio::test]
async fn idle_provider_status_retains_the_unavailable_contract_and_google_rule(
	mut authority_store: Authority,
	#[case] issuer: &str,
	#[case] allowed: bool,
) {
	authority_store.mapping = true;
	authority_store.issuer = Some(issuer.into());
	authority_store.validated = Some(now() - Duration::minutes(15));
	let result = authority(&authority_store, None).await;
	if allowed {
		assert!(result.is_ok());
	} else {
		assert!(matches!(result, Err(Error::IdentityStatusUnavailable)));
	}
}
#[rstest]
#[tokio::test]
async fn mappings_absent_from_the_snapshot_do_not_require_dashboard_status(
	mut authority_store: Authority,
) {
	authority_store.issuer = None;
	authority_store.validated = None;
	authority(&authority_store, None).await.unwrap();
}
#[rstest]
#[case::disabled_subject(0)]
#[case::disabled_parent(1)]
#[case::missing_subject(2)]
#[case::malformed_bundle(3)]
#[tokio::test]
async fn invalid_or_revoked_subject_authority_denies_idle_streams(
	mut authority_store: Authority,
	#[case] denied: u8,
) {
	match denied {
		0 => authority_store.document["subjects"]["reader"]["enabled"] = json!(false),
		1 => {
			authority_store.document["subjects"]["reader"]["delegated_by"] = json!("parent");
			authority_store.document["subjects"]["parent"]["enabled"] = json!(false);
		}
		2 => {
			authority_store.document["subjects"]
				.as_object_mut()
				.unwrap()
				.remove("reader");
		}
		_ => authority_store.document["tenant"] = json!("invalid tenant"),
	};
	assert!(matches!(
		authority(&authority_store, None).await,
		Err(Error::Forbidden)
	));
}
#[rstest]
#[case::read("workspace.read")]
#[case::events("workspace.events")]
#[tokio::test]
async fn selected_stream_requires_both_current_workspace_actions(
	mut authority_store: Authority,
	#[case] omitted: &str,
) {
	let actions = authority_store.document["policies"][0]["actions"]
		.as_array_mut()
		.unwrap();
	actions.retain(|value| value != omitted);
	assert!(matches!(
		authority(&authority_store, Some(Uuid::from_u128(1))).await,
		Err(Error::Forbidden)
	));
}
#[rstest]
#[tokio::test]
async fn absent_ownership_denies_selected_streams_but_does_not_expand_global_idle_checks(
	mut authority_store: Authority,
) {
	authority_store.owner = None;
	assert!(matches!(
		authority(&authority_store, Some(Uuid::from_u128(1))).await,
		Err(Error::Forbidden)
	));
	authority(&authority_store, None).await.unwrap();
}
#[rstest]
#[tokio::test]
async fn malformed_snapshot_json_keeps_its_error_identity(mut authority_store: Authority) {
	authority_store.document = json!("invalid");
	assert!(matches!(
		authority(&authority_store, None).await,
		Err(Error::Json(_))
	));
}
#[rstest]
#[tokio::test]
async fn snapshot_adapter_failure_keeps_its_opaque_identity(mut authority_store: Authority) {
	authority_store.fail = true;
	let Error::Port(error) = authority(&authority_store, None).await.unwrap_err() else {
		panic!("expected opaque error");
	};
	assert!(error.is::<std::io::Error>());
}
struct Session {
	events: Vec<Event>,
	visible: Vec<Uuid>,
	event_workspaces: Vec<Uuid>,
	calls: Vec<String>,
	queries: Vec<(i64, usize, Option<Uuid>, Vec<Uuid>)>,
	deny_action: Option<&'static str>,
	denied_workspace: Option<Uuid>,
	fail: Option<&'static str>,
}
#[fixture]
fn session() -> Session {
	Session {
		events: vec![],
		visible: vec![Uuid::from_u128(3), Uuid::from_u128(2), Uuid::from_u128(1)],
		event_workspaces: vec![Uuid::from_u128(1)],
		calls: vec![],
		queries: vec![],
		deny_action: None,
		denied_workspace: Some(Uuid::from_u128(2)),
		fail: None,
	}
}
impl Session {
	fn touch(&mut self, call: &'static str) -> Result<()> {
		self.calls.push(call.into());
		if self.fail == Some(call) {
			Err(Error::Port(Box::new(std::io::Error::other(call))))
		} else {
			Ok(())
		}
	}
}
fn event(sequence: i64, visible: bool) -> Event {
	Event {
		sequence,
		id: Uuid::from_u128(sequence as u128),
		node_id: "aidash://local".into(),
		workspace_id: Some(Uuid::from_u128(1)),
		kind: "task.created".into(),
		data: json!({"visible":visible}),
		created_at: now(),
	}
}
#[async_trait]
impl StreamSession for Session {
	async fn event_workspaces(&mut self, _: Option<Uuid>) -> Result<Vec<Uuid>> {
		self.touch("event_workspaces")?;
		Ok(self.event_workspaces.clone())
	}
	async fn require_workspace(&mut self, _: Uuid, action: &str) -> Result<()> {
		self.touch("require")?;
		self.calls.push(action.into());
		if self.deny_action == Some(action) {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
	async fn visible(&mut self, action: &str) -> Result<Vec<Uuid>> {
		assert_eq!(action, "workspace.read");
		self.touch("visible")?;
		Ok(self.visible.clone())
	}
	async fn allowed(&mut self, workspace: Uuid, action: &str) -> Result<bool> {
		assert_eq!(action, "workspace.events");
		self.touch("allowed")?;
		Ok(self.denied_workspace != Some(workspace))
	}
	async fn stream_rows(
		&mut self,
		after: i64,
		workspace: Option<Uuid>,
		visible: &[Uuid],
	) -> Result<Vec<Event>> {
		self.touch("stream_rows")?;
		self.queries.push((after, 100, workspace, visible.to_vec()));
		Ok(self
			.events
			.iter()
			.filter(|row| row.sequence > after.max(0))
			.take(100)
			.cloned()
			.collect())
	}
	async fn read_rows(
		&mut self,
		cursor: i64,
		workspace: Option<Uuid>,
		visible: &[Uuid],
		page_size: usize,
	) -> Result<Vec<Event>> {
		self.touch("read_rows")?;
		self.queries
			.push((cursor, page_size, workspace, visible.to_vec()));
		Ok(self
			.events
			.iter()
			.filter(|row| row.sequence > cursor)
			.take(page_size)
			.cloned()
			.collect())
	}
	async fn event_visible(&mut self, event: &Event) -> Result<bool> {
		self.touch("event_visible")?;
		Ok(event.data["visible"] == true)
	}
}
#[rstest]
#[case::empty(0,-5,false)]
#[case::short(17, 17, false)]
#[case::full(100, 100, true)]
#[case::more(101, 100, true)]
#[tokio::test]
async fn stream_page_cursor_advances_over_hidden_rows_and_keeps_candidate_fullness(
	mut session: Session,
	#[case] count: i64,
	#[case] scanned: i64,
	#[case] more: bool,
) {
	session.events = (1..=count).map(|seq| event(seq, false)).collect();
	let queried = Cell::new(0);
	let (events, cursor, has_more) =
		stream_page(&mut session, -5, None, || queried.set(queried.get() + 1))
			.await
			.unwrap();
	assert!(events.is_empty());
	assert_eq!(cursor, scanned);
	assert_eq!(has_more, more);
	assert_eq!(queried.get(), 1);
	assert_eq!(session.queries.len(), 1);
}
#[rstest]
#[tokio::test]
async fn selected_stream_denial_precedes_query_notification_and_event_rows(mut session: Session) {
	session.event_workspaces.clear();
	let queried = Cell::new(false);
	assert!(matches!(
		stream_page(&mut session, 0, Some(Uuid::from_u128(1)), || queried
			.set(true))
		.await,
		Err(Error::Forbidden)
	));
	assert!(!queried.get());
	assert!(session.queries.is_empty());
	assert_eq!(session.calls, vec!["event_workspaces"]);
}
#[rstest]
#[tokio::test]
async fn replay_continues_past_hidden_pages_and_returns_the_last_scanned_event(
	mut session: Session,
) {
	session.events = (1..=1100).map(|seq| event(seq, seq >= 1050)).collect();
	let (rows, cursor) = read_events(&mut session, -9, Some(Uuid::from_u128(1)), 2)
		.await
		.unwrap();
	assert_eq!(
		rows.iter().map(|row| row.sequence).collect::<Vec<_>>(),
		vec![1050, 1051]
	);
	assert_eq!(cursor, 1051);
	assert_eq!(
		session.queries.iter().map(|q| q.0).collect::<Vec<_>>(),
		vec![0, 500, 1000]
	);
	assert_eq!(
		&session.calls[..4],
		&["require", "workspace.read", "require", "workspace.events"]
	);
}
#[rstest]
#[tokio::test]
async fn replay_scan_budget_includes_hidden_rows_and_keeps_an_advancing_resume_cursor(
	mut session: Session,
) {
	session.events = (1..=5000).map(|seq| event(seq, false)).collect();
	let (rows, cursor) = read_events(&mut session, 0, None, 1000).await.unwrap();
	assert!(rows.is_empty());
	assert_eq!(cursor, 4096);
	assert_eq!(session.queries.len(), 9);
	assert_eq!(session.queries[8].0, 4000);
	assert_eq!(session.queries[8].1, 96);
	assert_eq!(
		session.queries[0].3,
		vec![Uuid::from_u128(3), Uuid::from_u128(1)]
	);
}
#[rstest]
#[case::negative(-10,1)]
#[case::zero(0, 1)]
#[case::normal(5, 5)]
#[case::max(1000, 1000)]
#[case::over(5000, 1000)]
#[tokio::test]
async fn replay_keeps_the_existing_limit_bounds_and_sequence_order(
	mut session: Session,
	#[case] limit: i64,
	#[case] count: usize,
) {
	session.events = (1..=1100).map(|seq| event(seq, true)).collect();
	let (rows, cursor) = read_events(&mut session, 0, Some(Uuid::from_u128(1)), limit)
		.await
		.unwrap();
	assert_eq!(rows.len(), count);
	assert_eq!(cursor, count as i64);
	assert_eq!(rows[0].sequence, 1);
	assert_eq!(rows.last().unwrap().sequence, count as i64);
}
#[rstest]
#[case::read("workspace.read")]
#[case::events("workspace.events")]
#[tokio::test]
async fn replay_workspace_denial_keeps_queries_unstarted(
	mut session: Session,
	#[case] action: &'static str,
) {
	session.deny_action = Some(action);
	assert!(matches!(
		read_events(&mut session, 0, Some(Uuid::from_u128(1)), 1).await,
		Err(Error::Forbidden)
	));
	assert!(session.queries.is_empty());
	assert_eq!(session.calls.last().unwrap(), action);
}
#[rstest]
#[case::workspace_absent(0)]
#[case::workspace_denied(1)]
#[case::event_denied(2)]
#[case::permitted(3)]
#[tokio::test]
async fn each_buffered_frame_requires_current_workspace_and_event_authority(
	mut session: Session,
	#[case] state: u8,
) {
	let mut frame = event(1, state != 2);
	if state == 0 {
		frame.workspace_id = None;
	}
	if state == 1 {
		session.event_workspaces.clear();
	}
	assert_eq!(can_emit(&mut session, &frame).await.unwrap(), state == 3);
	assert_eq!(
		session.calls,
		match state {
			0 => vec![],
			1 => vec!["event_workspaces"],
			_ => vec!["event_workspaces", "event_visible"],
		}
	);
}
#[rstest]
#[case::global_ownership("visible")]
#[case::global_policy("allowed")]
#[case::rows("read_rows")]
#[case::event_policy("event_visible")]
#[tokio::test]
async fn replay_adapter_failures_keep_their_opaque_identity(
	mut session: Session,
	#[case] fail: &'static str,
) {
	session.events = vec![event(1, true)];
	session.fail = Some(fail);
	let Error::Port(error) = read_events(&mut session, 0, None, 1).await.unwrap_err() else {
		panic!("expected opaque error");
	};
	assert!(error.is::<std::io::Error>());
	assert_eq!(session.calls.last().unwrap(), fail);
}
