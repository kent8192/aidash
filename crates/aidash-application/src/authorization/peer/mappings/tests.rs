use super::*;
use crate::tenancy::TenancyScope;
use aidash_domain::nodes::DeploymentId;
use async_trait::async_trait;
use chrono::DateTime;
use rstest::{fixture, rstest};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use uuid::Uuid;
const RECEIVER: &str = "calm-heron-ab12cd34";
const SOURCE: &str = "aidash://remote/quiet-otter-cd34ef56";
fn receiver() -> NodeName {
	NodeName::parse(RECEIVER).unwrap()
}
fn source() -> NodeAddress {
	NodeAddress::parse(SOURCE).unwrap()
}
fn tenant() -> TenantName {
	TenantName::new("tenant").unwrap()
}
#[fixture]
fn input() -> PeerMappingInput {
	PeerMappingInput {
		node: receiver(),
		source_node: source(),
		source_subject: "alice".into(),
		credential_id: Uuid::from_u128(1),
		enabled: true,
		expected_revision: 0,
	}
}
fn mapping() -> Mapping {
	Mapping {
		node: receiver(),
		source_node: source(),
		source_subject: "alice".into(),
		tenant: "tenant".into(),
		credential_id: Uuid::from_u128(1),
		enabled: true,
		revision: 7,
		actor: "deployment_administrator".into(),
		updated_at: DateTime::from_timestamp(1000, 0).unwrap(),
	}
}
#[derive(Clone)]
struct Repository {
	calls: Arc<Mutex<Vec<&'static str>>>,
	credential_denied: bool,
	not_exportable: bool,
	changed: bool,
	registered: bool,
	history_failed: bool,
}
#[fixture]
fn repository() -> Repository {
	Repository {
		calls: Arc::new(Mutex::new(vec![])),
		credential_denied: false,
		not_exportable: false,
		changed: false,
		registered: true,
		history_failed: false,
	}
}
impl Repository {
	fn called(&self, call: &'static str) {
		self.calls.lock().unwrap().push(call)
	}
	fn calls(&self) -> Vec<&'static str> {
		self.calls.lock().unwrap().clone()
	}
}
struct Write {
	repository: Repository,
}
struct Access {
	repository: Repository,
	environment: Value,
	context: Value,
}
#[async_trait]
impl MappingRepository for Repository {
	type Write = Write;
	type Access = Access;
	fn deployment(&self) -> &DeploymentId {
		static OWN: std::sync::LazyLock<DeploymentId> =
			std::sync::LazyLock::new(|| DeploymentId::parse("aidash://local").unwrap());
		&OWN
	}
	async fn begin_write(&self, scope: &TenancyScope, enabled: bool) -> Result<Write> {
		assert!(
			scope.tenant_name().is_some(),
			"a mapping is written in its Tenant's scope"
		);
		self.called(if enabled {
			"begin-data"
		} else {
			"begin-control"
		});
		Ok(Write {
			repository: self.clone(),
		})
	}
	async fn resolved(
		&self,
		tenant: &TenantName,
		node: &NodeName,
		source: &NodeAddress,
		subject: &str,
	) -> Result<Option<Mapping>> {
		assert_eq!(
			(
				tenant.as_str(),
				node.as_str(),
				source.to_string().as_str(),
				subject
			),
			("tenant", RECEIVER, SOURCE, "alice")
		);
		self.called("resolve");
		Ok(Some(mapping()))
	}
	async fn credential_subject(&self, _: &Mapping) -> Result<Option<String>> {
		self.called("resolve-credential");
		Ok(Some("local-subject".into()))
	}
	async fn begin_access(&self, _: &Mapping, subject: &str, exclusive: bool) -> Result<Access> {
		assert_eq!(subject, "local-subject");
		assert!(exclusive);
		self.called("begin-authority");
		if self.credential_denied {
			return Err(Error::Unauthorized);
		}
		Ok(Access {
			repository: self.clone(),
			environment: json!({"transport":"old","trusted":"fact"}),
			context: Value::Null,
		})
	}
	async fn begin_subject(&self, _: &TenantName, subject: &str, _: bool) -> Result<Access> {
		self.called("begin-subject");
		assert_eq!(subject, "alice");
		Ok(Access {
			repository: self.clone(),
			environment: json!({"transport":"old"}),
			context: Value::Null,
		})
	}
}
#[async_trait]
impl MappingWrite for Write {
	async fn policy(&mut self, tenant: &str) -> Result<()> {
		assert_eq!(tenant, "tenant");
		self.repository.called("policy");
		Ok(())
	}
	async fn credential_subject(&mut self, _: &str, _: Uuid) -> Result<Option<String>> {
		self.repository.called("credential-subject");
		Ok(Some("local-subject".into()))
	}
	async fn enabled_registration(&mut self, _: &str, _: &NodeName, _: &NodeAddress) -> Result<()> {
		self.repository.called("enabled-registration");
		Ok(())
	}
	async fn exportable(&mut self, _: &str, _: Uuid) -> Result<()> {
		self.repository.called("exportable");
		if self.repository.not_exportable {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
	async fn lock_credential(&mut self, _: &str, _: Uuid, _: &str) -> Result<()> {
		self.repository.called("lock-credential");
		if self.repository.credential_denied {
			Err(Error::Unauthorized)
		} else {
			Ok(())
		}
	}
	async fn insert(&mut self, _: &str, _: &PeerMappingInput) -> Result<Option<Mapping>> {
		self.repository.called("insert");
		Ok(Some(mapping()))
	}
	async fn update(&mut self, _: &str, _: &PeerMappingInput) -> Result<Option<Mapping>> {
		self.repository.called("update");
		Ok(Some(mapping()))
	}
	async fn history(&mut self, _: &Mapping) -> Result<()> {
		self.repository.called("history");
		if self.repository.history_failed {
			Err(Error::External("history unavailable".into()))
		} else {
			Ok(())
		}
	}
	async fn commit(self) -> Result<()> {
		self.repository.called("commit");
		Ok(())
	}
}
#[async_trait]
impl MappingAccess for Access {
	async fn current(
		&mut self,
		_: &TenantName,
		_: &NodeName,
		_: &NodeAddress,
		_: &str,
	) -> Result<Option<Mapping>> {
		self.repository.called("lease-mapping");
		let mut current = mapping();
		if self.repository.changed {
			current.credential_id = Uuid::from_u128(2)
		}
		Ok(Some(current))
	}
	async fn registered(&mut self, _: &TenantName, _: &NodeName, _: &NodeAddress) -> Result<bool> {
		self.repository.called("lease-registration");
		Ok(self.repository.registered)
	}
	fn environment(&mut self) -> &mut Value {
		&mut self.environment
	}
	fn context(&mut self) -> &mut Value {
		&mut self.context
	}
}
#[rstest]
#[case(true, 0, "insert")]
#[case(true, 7, "update")]
#[case(false, 7, "update")]
#[tokio::test]
async fn writes_keep_lock_order_and_revocation_uses_control_scope(
	repository: Repository,
	mut input: PeerMappingInput,
	#[case] enabled: bool,
	#[case] revision: i64,
	#[case] mutation: &'static str,
) {
	input.enabled = enabled;
	input.expected_revision = revision;
	let result = write(&repository, "tenant", input).await.unwrap();
	assert_eq!(result, mapping());
	let mut expected = if enabled {
		vec![
			"begin-data",
			"policy",
			"credential-subject",
			"enabled-registration",
			"exportable",
			"lock-credential",
		]
	} else {
		vec![
			"begin-control",
			"policy",
			"credential-subject",
			"exportable",
		]
	};
	expected.extend([mutation, "history", "commit"]);
	assert_eq!(repository.calls(), expected);
}
#[rstest]
#[tokio::test]
async fn rejected_mapped_credential_is_forbidden_without_a_mutation(
	mut repository: Repository,
	input: PeerMappingInput,
) {
	repository.credential_denied = true;
	let result = write(&repository, "tenant", input).await;
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(
		repository.calls(),
		vec![
			"begin-data",
			"policy",
			"credential-subject",
			"enabled-registration",
			"exportable",
			"lock-credential"
		]
	);
}
#[rstest]
#[tokio::test]
async fn revocation_does_not_require_the_old_credential_to_remain_usable(
	mut repository: Repository,
	mut input: PeerMappingInput,
) {
	repository.credential_denied = true;
	input.enabled = false;
	input.expected_revision = 7;
	assert_eq!(
		write(&repository, "tenant", input).await.unwrap(),
		mapping()
	);
	assert_eq!(
		repository.calls(),
		vec![
			"begin-control",
			"policy",
			"credential-subject",
			"exportable",
			"update",
			"history",
			"commit"
		]
	);
}
#[rstest]
#[case(true)]
#[case(false)]
#[tokio::test]
async fn a_credential_that_cannot_be_exported_is_refused_enabled_or_not(
	mut repository: Repository,
	mut input: PeerMappingInput,
	#[case] enabled: bool,
) {
	repository.not_exportable = true;
	input.enabled = enabled;
	input.expected_revision = 7;
	assert!(matches!(
		write(&repository, "tenant", input).await,
		Err(Error::Forbidden)
	));
	assert_eq!(repository.calls().last(), Some(&"exportable"));
}
#[rstest]
#[tokio::test]
async fn failed_history_prevents_commit(mut repository: Repository, input: PeerMappingInput) {
	repository.history_failed = true;
	assert!(matches!(
		write(&repository, "tenant", input).await,
		Err(Error::External(_))
	));
	assert_eq!(
		repository.calls(),
		vec![
			"begin-data",
			"policy",
			"credential-subject",
			"enabled-registration",
			"exportable",
			"lock-credential",
			"insert",
			"history"
		]
	);
}
#[rstest]
#[tokio::test]
async fn disabling_a_new_mapping_does_not_acquire_a_write_scope(
	repository: Repository,
	mut input: PeerMappingInput,
) {
	input.enabled = false;
	assert!(matches!(
		write(&repository, "tenant", input).await,
		Err(Error::Domain(_))
	));
	assert!(repository.calls().is_empty());
}
#[rstest]
#[tokio::test]
async fn credential_rebinding_before_the_lease_is_rejected(mut repository: Repository) {
	repository.changed = true;
	assert!(matches!(
		mapped(
			&repository,
			&tenant(),
			&receiver(),
			&source(),
			"alice",
			true
		)
		.await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		repository.calls(),
		vec![
			"resolve",
			"resolve-credential",
			"begin-authority",
			"lease-mapping"
		]
	);
}
#[rstest]
#[tokio::test]
async fn unregistered_source_cannot_establish_mapped_authority(mut repository: Repository) {
	repository.registered = false;
	assert!(matches!(
		mapped(
			&repository,
			&tenant(),
			&receiver(),
			&source(),
			"alice",
			true
		)
		.await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		repository.calls(),
		vec![
			"resolve",
			"resolve-credential",
			"begin-authority",
			"lease-mapping",
			"lease-registration"
		]
	);
}
#[rstest]
#[tokio::test]
async fn unusable_credential_does_not_challenge_an_authenticated_peer(mut repository: Repository) {
	repository.credential_denied = true;
	assert!(matches!(
		mapped(
			&repository,
			&tenant(),
			&receiver(),
			&source(),
			"alice",
			true
		)
		.await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		repository.calls(),
		vec!["resolve", "resolve-credential", "begin-authority"]
	);
}
#[rstest]
#[tokio::test]
async fn admitted_context_retains_transport_facts_and_the_source_identity(repository: Repository) {
	let scope = mapped(
		&repository,
		&tenant(),
		&receiver(),
		&source(),
		"alice",
		true,
	)
	.await
	.unwrap();
	assert_eq!(
		scope.environment,
		json!({"transport":"federation","source_node":SOURCE,"source_scope":"remote","trusted":"fact"})
	);
	assert_eq!(
		scope.context,
		json!({"source_node":SOURCE,"source_subject":"alice"})
	);
}
