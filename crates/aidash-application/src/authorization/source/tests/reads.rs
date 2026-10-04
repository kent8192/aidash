use super::*;
use crate::{
	authorization::{
		source::reads as use_case,
		visits::{ReadVisit, ReadVisits},
	},
	ports::authorization::source::{
		SourcePeerScope,
		reads::{ProducerScope, SourceReadScope},
	},
};
use aidash_domain::federation::{
	Peer,
	dependencies::Reference,
	execution::home::{Grant, HomeBinding},
};
use chrono::{TimeZone, Utc};
struct Reader {
	base: Scope,
	grant: Option<Grant>,
	binding: Option<HomeBinding>,
	task: Task,
	reads: bool,
	frontier: Option<Vec<Reference>>,
	visits: ReadVisits,
	producer_entered: bool,
	blocked: bool,
	verified: Vec<Reference>,
	verified_visible: bool,
	output_reads: usize,
	disabled: bool,
}
struct Producer<'a> {
	base: &'a mut Scope,
	subjects: Option<Vec<String>>,
	context: Option<Value>,
	blocked: bool,
}
impl Drop for Producer<'_> {
	fn drop(&mut self) {
		self.base.subjects = self.subjects.take().unwrap();
		self.base.context = self.context.take().unwrap();
	}
}
impl Reader {
	fn new() -> Self {
		let inspection = inspection("model");
		let subjects = vec![
			"requester".into(),
			qualified_agent("aidash://receiver", "agent", "1.0.0"),
		];
		let grant = Grant {
			id: Uuid::from_u128(3),
			task_id: task().id,
			task_revision: task().revision,
			workspace_id: task().workspace_id,
			node_id: "aidash://receiver".into(),
			tenant: "tenant".into(),
			credential_id: Uuid::from_u128(4),
			root_subject: "requester".into(),
			subject_chain: subjects,
			inspection: serde_json::to_value(inspection).unwrap(),
			expires_at: Utc.timestamp_opt(3600, 0).unwrap(),
			revoked: false,
			semantic: json!({"mode":"disabled"}),
		};
		let binding = HomeBinding {
			grant_id: grant.id,
			admission_id: Uuid::from_u128(5),
			task_id: task().id,
			task_revision: task().revision,
			initial_task: json!(task()),
		};
		let mut base = Scope::new();
		base.subjects = vec!["viewer".into()];
		base.context = json!({"viewer":"context"});
		Self {
			base,
			grant: Some(grant),
			binding: Some(binding),
			task: task(),
			reads: true,
			frontier: Some(vec![]),
			visits: Default::default(),
			producer_entered: false,
			blocked: false,
			verified: vec![],
			verified_visible: true,
			output_reads: 0,
			disabled: false,
		}
	}
}
#[async_trait]
impl SourceAuthorityScope for Producer<'_> {
	fn source_subjects(&self) -> &[String] {
		self.base.source_subjects()
	}
	fn source_bundle(&self) -> &PolicyBundle {
		self.base.source_bundle()
	}
	fn source_context(&mut self, attributes: Value) {
		self.base.source_context(attributes)
	}
	fn source_resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.base.source_resource(kind, id, attributes)
	}
	async fn generation_home(
		&mut self,
		task: &Task,
		node: &str,
		generation: Option<&Value>,
	) -> Result<()> {
		self.base.generation_home(task, node, generation).await
	}
	async fn source_workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.base.source_workspace(id).await
	}
	async fn source_task_resource(&mut self, task: &Task) -> Result<Resource> {
		self.base.source_task_resource(task).await
	}
	async fn source_require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.base.source_require(resource, action).await
	}
}
#[async_trait]
impl ProducerScope for Producer<'_> {
	async fn producer_semantic_sources(&mut self, _: Uuid) -> Result<()> {
		if self.blocked {
			std::future::pending::<()>().await;
		}
		Ok(())
	}
}
#[async_trait]
impl SourcePeerScope for Reader {
	async fn peer(&mut self, node: &str) -> Result<Option<Peer>> {
		Ok(Some(Peer {
			node_id: node.into(),
			endpoint: "https://receiver.invalid".into(),
			credential_env: "CREDENTIAL".into(),
			protocol_version: "current".into(),
			enabled: true,
		}))
	}
}
fn visit_key() -> (String, Uuid, String) {
	(
		"aidash://home".into(),
		Uuid::from_u128(3),
		"grant:viewer".into(),
	)
}
#[async_trait]
impl SourceReadScope for Reader {
	fn output_visit(&self, _: Uuid) -> Option<ReadVisit> {
		self.visits.enter(visit_key())
	}
	fn protocol_version(&self) -> &str {
		"current"
	}
	async fn reader_grant(&mut self, _: &str, _: Uuid) -> Result<Option<Grant>> {
		Ok(self.grant.clone())
	}
	async fn read_binding(&mut self, _: Uuid) -> Result<Option<HomeBinding>> {
		Ok(self.binding.clone())
	}
	async fn read_task(&mut self, _: Uuid) -> Result<Task> {
		Ok(self.task.clone())
	}
	async fn grant_reads(&mut self, _: Uuid) -> Result<bool> {
		Ok(self.reads)
	}
	async fn producer<'a>(&'a mut self, grant: &Grant) -> Result<Box<dyn ProducerScope + 'a>> {
		self.producer_entered = true;
		let subjects = std::mem::replace(&mut self.base.subjects, grant.subject_chain.clone());
		let context = self.base.context.clone();
		Ok(Box::new(Producer {
			base: &mut self.base,
			subjects: Some(subjects),
			context: Some(context),
			blocked: self.blocked,
		}))
	}
	fn record_admission(&mut self, node: &str, grant: Uuid, admission: Uuid) -> Result<()> {
		self.frontier
			.as_mut()
			.ok_or(Error::Forbidden)?
			.push(Reference::Admission {
				node_id: node.into(),
				home_node: "aidash://home".into(),
				grant_id: grant,
				admission_id: admission,
			});
		Ok(())
	}
	async fn output_record(&mut self, _: Uuid) -> Result<Option<(String, Value)>> {
		self.output_reads += 1;
		Ok(Some((
			"aidash://receiver".into(),
			if self.disabled {
				json!({"mode":"disabled"})
			} else {
				json!({"mode":"required_home","home_lineage":[],"execution_lineage":[],"version":1,"index_revision":1,"index_digest":"index","embedding":{"node_id":"aidash://home","entry":{"id":"embedding","version":"1"},"digest":"digest","configuration_digest":"config"}})
			},
		)))
	}
	fn collecting_dependencies(&self) -> bool {
		self.frontier.is_some()
	}
	fn start_dependencies(&mut self) {
		self.frontier = Some(vec![]);
	}
	fn take_dependencies(&mut self) -> Vec<Reference> {
		self.frontier.take().unwrap_or_default()
	}
	async fn verify_dependencies(&mut self, pending: Vec<Reference>) -> Result<bool> {
		self.verified = pending;
		Ok(self.verified_visible)
	}
}
#[rstest]
#[case("grant")]
#[case("binding")]
#[case("admission")]
#[case("task")]
#[case("revision")]
#[case("membership")]
#[tokio::test]
async fn current_reader_binding_and_revision_precede_original_producer_checks(
	#[case] change: &str,
) {
	let mut reader = Reader::new();
	match change {
		"grant" => reader.grant = None,
		"binding" => reader.binding = None,
		"admission" => reader.binding.as_mut().unwrap().admission_id = Uuid::from_u128(99),
		"task" => reader.binding.as_mut().unwrap().task_id = Uuid::from_u128(99),
		"revision" => reader.task.revision += 1,
		"membership" => reader.reads = false,
		_ => panic!("unknown authority"),
	}
	assert!(
		!use_case::visible(
			&mut reader,
			"aidash://receiver",
			Uuid::from_u128(3),
			Uuid::from_u128(5)
		)
		.await
		.unwrap()
	);
	assert!(!reader.producer_entered);
	assert_eq!(reader.base.subjects, vec!["viewer"]);
}
#[rstest]
#[tokio::test]
async fn rejected_producer_authority_restores_current_viewer_context() {
	let mut reader = Reader::new();
	reader.base.denied = Some("task.execute");
	assert!(matches!(
		use_case::visible(
			&mut reader,
			"aidash://receiver",
			Uuid::from_u128(3),
			Uuid::from_u128(5)
		)
		.await,
		Err(Error::Forbidden)
	));
	assert!(reader.producer_entered);
	assert_eq!(reader.base.subjects, vec!["viewer"]);
	assert_eq!(reader.base.context, json!({"viewer":"context"}));
	assert!(reader.frontier.unwrap().is_empty());
}
#[rstest]
#[tokio::test]
async fn malformed_producer_snapshot_restores_viewer_before_error_propagation() {
	let mut reader = Reader::new();
	reader.grant.as_mut().unwrap().inspection = json!({"invalid":true});
	assert!(matches!(
		use_case::visible(
			&mut reader,
			"aidash://receiver",
			Uuid::from_u128(3),
			Uuid::from_u128(5)
		)
		.await,
		Err(Error::Json(_))
	));
	assert_eq!(reader.base.subjects, vec!["viewer"]);
	assert_eq!(reader.base.context, json!({"viewer":"context"}));
}
#[rstest]
#[tokio::test]
async fn cancelled_producer_source_check_restores_viewer_via_guard_drop() {
	let mut reader = Reader::new();
	reader.blocked = true;
	let mut future = Box::pin(use_case::visible(
		&mut reader,
		"aidash://receiver",
		Uuid::from_u128(3),
		Uuid::from_u128(5),
	));
	assert!(futures_util::poll!(&mut future).is_pending());
	// Dropping the pending request cancels it; the producer frame must still restore the viewer.
	drop(future);
	assert_eq!(reader.base.subjects, vec!["viewer"]);
	assert_eq!(reader.base.context, json!({"viewer":"context"}));
}
#[rstest]
#[tokio::test]
async fn authorized_source_leaf_records_only_the_exact_admission_dependency() {
	let mut reader = Reader::new();
	assert!(
		use_case::visible(
			&mut reader,
			"aidash://receiver",
			Uuid::from_u128(3),
			Uuid::from_u128(5)
		)
		.await
		.unwrap()
	);
	assert_eq!(
		reader.frontier,
		Some(vec![Reference::Admission {
			node_id: "aidash://receiver".into(),
			home_node: "aidash://home".into(),
			grant_id: Uuid::from_u128(3),
			admission_id: Uuid::from_u128(5)
		}])
	);
	assert_eq!(reader.base.subjects, vec!["viewer"]);
}
#[rstest]
#[tokio::test]
async fn output_coordinator_verifies_foreign_dependencies_before_visibility() {
	let mut reader = Reader::new();
	reader.frontier = None;
	reader.verified_visible = false;
	assert!(
		!use_case::output_visible(&mut reader, Uuid::from_u128(3))
			.await
			.unwrap()
	);
	assert_eq!(reader.verified.len(), 1);
	assert!(reader.frontier.is_none());
}
#[rstest]
#[tokio::test]
async fn disabled_semantic_output_uses_current_membership_without_producer_or_foreign_checks() {
	let mut reader = Reader::new();
	reader.disabled = true;
	reader.reads = false;
	assert!(
		!use_case::output_visible(&mut reader, Uuid::from_u128(3))
			.await
			.unwrap()
	);
	assert!(!reader.producer_entered);
	assert!(reader.verified.is_empty());
}
#[rstest]
#[tokio::test]
async fn already_active_output_visit_does_not_start_recursive_disclosure() {
	let mut reader = Reader::new();
	let _guard = reader.visits.enter(visit_key()).unwrap();
	assert!(
		use_case::output_visible(&mut reader, Uuid::from_u128(3))
			.await
			.unwrap()
	);
	assert_eq!(reader.output_reads, 0);
	assert!(!reader.producer_entered);
}
