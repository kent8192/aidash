use super::*;
use aidash_domain::registry::workbench::{
	incident::Incident, inspection::Inspection, permissions::PermissionContext,
};
use async_trait::async_trait;
use chrono::TimeZone;
use rstest::rstest;
use serde_json::{Value, json};
use std::sync::Mutex;
use uuid::Uuid;
#[derive(Default)]
struct State {
	calls: Vec<&'static str>,
	context: Option<Value>,
}
struct Sources {
	principal: Principal,
	state: Mutex<State>,
	failed: Option<&'static str>,
}
impl Sources {
	fn new(principal: Principal) -> Self {
		Self {
			principal,
			state: Mutex::new(State::default()),
			failed: None,
		}
	}
}
fn reference() -> EntityRef {
	EntityRef {
		id: "agent".into(),
		version: "1".into(),
	}
}
fn query() -> ReportQuery {
	ReportQuery {
		format: None,
		include_sensitive: false,
		tenant: None,
		subject: None,
		workspace_id: None,
	}
}
fn subject() -> Principal {
	Principal::Subject {
		tenant: "tenant".into(),
		subject: "reader".into(),
	}
}
fn observation() -> Inspection {
	Inspection {
		entry: serde_json::from_value(
			json!({"id":"agent","version":"1","kind":"agent","name":{},"description":{}}),
		)
		.unwrap(),
		source_node: "node".into(),
		observed_at: Utc.timestamp_opt(100, 0).unwrap(),
		workspaces: vec![],
		usage_truncated: false,
		test_evidence: vec![],
		test_evidence_truncated: false,
		external_assessment_available: false,
	}
}
fn reported_incident() -> Incident {
	Incident {
		id: Uuid::from_u128(1),
		tenant: "tenant".into(),
		agent_id: "agent".into(),
		version: "1".into(),
		revision: 7,
		severity: "high".into(),
		status: "open".into(),
		archived: false,
		owner: "owner".into(),
		notes: "source notes".into(),
		evidence: json!([{"title":"source","content":"copied private text","sha256":"digest","recorded_at":"time","other":"retained"},"untouched"]),
		created_at: Utc.timestamp_opt(10, 0).unwrap(),
		updated_at: Utc.timestamp_opt(20, 0).unwrap(),
		resolved_at: None,
		evidence_expires_at: None,
		evidence_expired_at: None,
	}
}
#[async_trait]
impl ReportSources for Sources {
	fn principal(&self) -> Principal {
		self.principal.clone()
	}
	async fn inspect(&self, entry: &EntityRef) -> Result<Inspection> {
		assert_eq!(*entry, reference());
		self.state.lock().unwrap().calls.push("inspection");
		if self.failed == Some("inspection") {
			return Err(Error::Forbidden);
		}
		Ok(observation())
	}
	async fn permission_context(
		&self,
		entry: &EntityRef,
		input: PermissionInput,
	) -> Result<PermissionContext> {
		assert_eq!(*entry, reference());
		let mut s = self.state.lock().unwrap();
		s.calls.push("permissions");
		s.context = Some(
			json!({"tenant":input.tenant,"subject":input.subject,"workspace_id":input.workspace_id}),
		);
		if self.failed == Some("permissions") {
			return Err(Error::Forbidden);
		}
		Ok(PermissionContext {
			tenant: input.tenant,
			subject: input.subject,
			workspace_id: input.workspace_id,
			policy_revision: 9,
			observed_at: Utc.timestamp_opt(30, 0).unwrap(),
			requested_capabilities: vec![],
			rows: vec![],
			workspace_read: None,
			note: "component context".into(),
		})
	}
	async fn incidents(&self, entry: &EntityRef) -> Result<Vec<Incident>> {
		assert_eq!(*entry, reference());
		self.state.lock().unwrap().calls.push("incidents");
		if self.failed == Some("incidents") {
			return Err(Error::External("incident source unavailable".into()));
		}
		Ok(vec![reported_incident()])
	}
}
#[rstest]
#[case(None, Format::Json)]
#[case(Some("json"), Format::Json)]
#[case(Some("html"), Format::Html)]
#[tokio::test]
async fn accepted_formats_share_the_same_independently_authorized_factual_sources(
	#[case] format: Option<&str>,
	#[case] expected: Format,
) {
	let sources = Sources::new(subject());
	let mut input = query();
	input.format = format.map(str::to_owned);
	let before = Utc::now();
	let result = assemble(&sources, reference(), input).await.unwrap();
	assert_eq!(result.format, expected);
	assert_eq!(result.report.inspection.source_node, "node");
	assert_eq!(result.report.permission_context.unwrap().policy_revision, 9);
	assert!(result.report.exported_at >= before);
	assert_eq!(
		sources.state.lock().unwrap().calls,
		vec!["inspection", "permissions", "incidents"]
	);
}
#[rstest]
#[tokio::test]
async fn invalid_format_precedes_any_source_or_authority_reads() {
	let sources = Sources::new(subject());
	let mut input = query();
	input.format = Some("pdf".into());
	assert!(
		matches!(assemble(&sources,reference(),input).await,Err(Error::Invalid(ref text)) if text=="report format must be json or html")
	);
	assert!(sources.state.lock().unwrap().calls.is_empty());
}
#[rstest]
#[case(Some("other"), None)]
#[case(None, Some("other"))]
#[tokio::test]
async fn subject_context_cannot_be_overridden_after_the_authorized_inspection(
	#[case] tenant: Option<&str>,
	#[case] selected_subject: Option<&str>,
) {
	let sources = Sources::new(subject());
	let mut input = query();
	input.tenant = tenant.map(str::to_owned);
	input.subject = selected_subject.map(str::to_owned);
	assert!(matches!(
		assemble(&sources, reference(), input).await,
		Err(Error::Forbidden)
	));
	assert_eq!(sources.state.lock().unwrap().calls, vec!["inspection"]);
}
#[rstest]
#[case(None, None, false, false)]
#[case(Some("tenant"), Some("reader"), true, true)]
#[case(Some("tenant"), None, false, false)]
#[case(None, Some("reader"), false, false)]
#[case(None, None, true, false)]
#[tokio::test]
async fn operator_context_requires_the_complete_identity_pair(
	#[case] tenant: Option<&str>,
	#[case] selected_subject: Option<&str>,
	#[case] workspace: bool,
	#[case] context: bool,
) {
	let sources = Sources::new(Principal::Operator);
	let mut input = query();
	input.tenant = tenant.map(str::to_owned);
	input.subject = selected_subject.map(str::to_owned);
	input.workspace_id = workspace.then(|| Uuid::from_u128(9));
	let result = assemble(&sources, reference(), input).await;
	let valid = context || tenant.is_none() && selected_subject.is_none() && !workspace;
	assert_eq!(result.is_ok(), valid);
	let s = sources.state.lock().unwrap();
	if valid {
		assert_eq!(result.unwrap().report.permission_context.is_some(), context);
		assert_eq!(s.context.is_some(), context);
	} else {
		assert!(matches!(result, Err(Error::Invalid(_))));
		assert_eq!(s.calls, vec!["inspection"]);
	}
}
#[rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn sensitivity_controls_only_the_copied_payload_content(#[case] sensitive: bool) {
	let sources = Sources::new(subject());
	let mut input = query();
	input.include_sensitive = sensitive;
	let result = assemble(&sources, reference(), input).await.unwrap();
	let incident = &result.report.incidents[0];
	let copy = incident.evidence[0].as_object().unwrap();
	assert_eq!(copy.contains_key("content"), sensitive);
	if sensitive {
		assert_eq!(copy["content"], "copied private text");
	}
	assert_eq!(copy["sha256"], "digest");
	assert_eq!(copy["recorded_at"], "time");
	assert_eq!(copy["title"], "source");
	assert_eq!(copy["other"], "retained");
	assert_eq!(incident.evidence[1], "untouched");
	assert_eq!(incident.notes, "source notes");
	assert_eq!(incident.revision, 7);
}
#[rstest]
#[case("inspection",vec!["inspection"])]
#[case("permissions",vec!["inspection","permissions"])]
#[case("incidents",vec!["inspection","permissions","incidents"])]
#[tokio::test]
async fn source_denials_and_failures_are_not_replaced_with_partial_reports(
	#[case] failed: &'static str,
	#[case] calls: Vec<&'static str>,
) {
	let mut sources = Sources::new(subject());
	sources.failed = Some(failed);
	assert!(assemble(&sources, reference(), query()).await.is_err());
	assert_eq!(sources.state.lock().unwrap().calls, calls);
}
