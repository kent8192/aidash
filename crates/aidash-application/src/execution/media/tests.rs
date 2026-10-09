use super::*;
use crate::ports::execution::media::{MediaAttachment, MediaTransaction};
use aidash_domain::{RunControl, RunPhase, model::MediaRouteEvidence};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use rstest::{fixture, rstest};
use serde_json::Value;
use std::{
	collections::BTreeMap,
	sync::{Arc, Mutex},
};
fn id(value: u128) -> Uuid {
	Uuid::from_u128(value)
}
fn attachment(filename: &str, media_type: &str, content: Vec<u8>) -> MediaAttachment {
	MediaAttachment {
		filename: filename.into(),
		media_type: media_type.into(),
		sha256: format!("{:x}", Sha256::digest(&content)),
		size_bytes: content.len() as i64,
		content,
	}
}
fn image() -> MediaAttachment {
	attachment(
		"saved.png",
		"image/png",
		b"\x89PNG\r\n\x1a\nfixture".to_vec(),
	)
}
fn audio() -> MediaAttachment {
	attachment("saved.mp3", "audio/mpeg", b"ID3fixture".to_vec())
}
fn routes(formats: Vec<String>) -> MediaRouteEvidence {
	MediaRouteEvidence {
		tag: "approved-route".into(),
		formats,
		source: "saved evidence".into(),
		verified_at: Utc::now() - Duration::hours(1),
		expires_at: Utc::now() + Duration::hours(1),
	}
}
#[fixture]
fn model() -> ModelConfig {
	ModelConfig {
		provider: "openrouter".into(),
		model_id: "saved-model".into(),
		endpoint: "https://provider.example/api".into(),
		credential_env: None,
		request_timeout_secs: None,
		reasoning_effort: None,
		context_window: 128000,
		max_output_tokens: Some(8192),
		modalities: vec!["text".into(), "image".into(), "audio".into()],
		media_routes: vec![routes(vec!["image/png".into(), "mp3".into()])],
		cost: json!({}),
		projection_versions: aidash_domain::context::projection::ProjectionVersion::legacy_only(),
		cache_mode: Default::default(),
	}
}
#[derive(Default)]
struct Scope {
	rows: BTreeMap<Uuid, Vec<MediaAttachment>>,
	calls: Vec<(&'static str, Uuid)>,
	denied: Option<Uuid>,
	fail: Option<Uuid>,
}
#[fixture]
fn scope() -> Scope {
	Scope {
		rows: BTreeMap::from([(id(1), vec![image(), audio()])]),
		..Scope::default()
	}
}
#[async_trait]
impl MediaAttachments for Scope {
	async fn attachments(
		&mut self,
		workspace: Uuid,
		message: Uuid,
	) -> Result<Vec<MediaAttachment>> {
		assert_eq!(workspace, id(10));
		self.calls.push(("attachments", message));
		if self.fail == Some(message) {
			return Err(Error::Port(Box::new(std::io::Error::other(
				"attachment fault",
			))));
		}
		Ok(self.rows.get(&message).cloned().unwrap_or_default())
	}
}
#[async_trait]
impl AuthorizedMediaScope for Scope {
	async fn authorize_message(&mut self, workspace: Uuid, message: Uuid) -> Result<()> {
		assert_eq!(workspace, id(10));
		self.calls.push(("authorize", message));
		if self.denied == Some(message) {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
}
fn parts(parts: &[ContentPart]) -> Vec<Value> {
	parts
		.iter()
		.map(|part| match part {
			ContentPart::Text(text) => json!({"text":text}),
			ContentPart::Image { media_type, bytes } => json!({"image":media_type,"bytes":bytes}),
			ContentPart::Audio { format, bytes } => json!({"audio":format,"bytes":bytes}),
		})
		.collect()
}
#[rstest]
#[tokio::test]
async fn selected_messages_are_all_authorized_before_ordered_attachment_bytes(
	mut scope: Scope,
	model: ModelConfig,
) {
	scope.rows.insert(id(2), vec![audio()]);
	let output = authorized(
		&mut scope,
		id(10),
		&[(7, id(1), 128000), (9, id(2), 128000)],
		&model,
	)
	.await
	.unwrap();
	assert_eq!(output.through_seq, Some(9));
	assert!(!output.has_more);
	assert_eq!(
		parts(&output.parts),
		vec![
			json!({"text":"Run message 7 attachment: saved.png"}),
			json!({"image":"image/png","bytes":image().content}),
			json!({"text":"Run message 7 attachment: saved.mp3"}),
			json!({"audio":"mp3","bytes":audio().content}),
			json!({"text":"Run message 9 attachment: saved.mp3"}),
			json!({"audio":"mp3","bytes":audio().content})
		]
	);
	assert_eq!(
		scope.calls,
		vec![
			("authorize", id(1)),
			("authorize", id(2)),
			("attachments", id(1)),
			("attachments", id(2))
		]
	);
}
#[rstest]
#[tokio::test]
async fn later_message_denial_prevents_reading_any_attachment_bytes(
	mut scope: Scope,
	model: ModelConfig,
) {
	scope.denied = Some(id(2));
	assert!(matches!(
		authorized(
			&mut scope,
			id(10),
			&[(7, id(1), 128000), (9, id(2), 128000)],
			&model
		)
		.await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		scope.calls,
		vec![("authorize", id(1)), ("authorize", id(2))]
	);
}
#[rstest]
#[tokio::test]
async fn empty_message_lists_have_no_reads_or_cursor(mut scope: Scope, model: ModelConfig) {
	let output = authorized(&mut scope, id(10), &[], &model).await.unwrap();
	assert!(output.parts.is_empty());
	assert_eq!(output.through_seq, None);
	assert!(!output.has_more);
	assert!(scope.calls.is_empty());
}
#[rstest]
#[tokio::test]
async fn messages_without_media_still_advance_the_last_complete_sequence(
	mut scope: Scope,
	model: ModelConfig,
) {
	scope.rows.clear();
	let output = load(
		&mut scope,
		id(10),
		&[(3, id(1), 128000), (8, id(2), 128000)],
		&model,
	)
	.await
	.unwrap();
	assert!(output.parts.is_empty());
	assert_eq!(output.through_seq, Some(8));
	assert!(!output.has_more);
	assert_eq!(
		scope.calls,
		vec![("attachments", id(1)), ("attachments", id(2))]
	);
}
#[rstest]
#[case::count(true)]
#[case::bytes(false)]
#[tokio::test]
async fn an_individual_message_over_the_limit_is_rejected_before_media_validation(
	mut scope: Scope,
	model: ModelConfig,
	#[case] count: bool,
) {
	let records = if count {
		vec![image(); 9]
	} else {
		vec![attachment(
			"too-large.png",
			"image/png",
			vec![0; 8 * 1024 * 1024 + 1],
		)]
	};
	scope.rows.insert(id(1), records);
	let error = load(&mut scope, id(10), &[(1, id(1), 128000)], &model)
		.await
		.err()
		.unwrap();
	assert!(
		matches!(error,Error::Invalid(ref message) if message=="run media input exceeds count or byte limit")
	);
}
#[rstest]
#[case::count(true)]
#[case::bytes(false)]
#[tokio::test]
async fn total_limits_defer_the_whole_next_message_without_examining_its_integrity(
	mut scope: Scope,
	model: ModelConfig,
	#[case] count: bool,
) {
	let first = if count {
		vec![image(); 8]
	} else {
		let mut bytes = vec![0; 8 * 1024 * 1024];
		bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
		vec![attachment("large.png", "image/png", bytes)]
	};
	scope.rows.insert(id(1), first.clone());
	let mut next = image();
	next.sha256 = "corrupt deferred record".into();
	scope.rows.insert(id(2), vec![next]);
	let output = load(
		&mut scope,
		id(10),
		&[(1, id(1), 128000), (2, id(2), 128000)],
		&model,
	)
	.await
	.unwrap();
	assert_eq!(output.parts.len(), first.len() * 2);
	assert_eq!(output.through_seq, Some(1));
	assert!(output.has_more);
	assert_eq!(
		scope.calls,
		vec![("attachments", id(1)), ("attachments", id(2))]
	);
}
#[rstest]
#[case::length(true)]
#[case::digest(false)]
#[tokio::test]
async fn immutable_attachment_integrity_is_required_even_after_a_complete_message(
	mut scope: Scope,
	model: ModelConfig,
	#[case] length: bool,
) {
	scope.rows.insert(id(1), vec![image()]);
	let mut corrupt = audio();
	if length {
		corrupt.size_bytes += 1;
	} else {
		corrupt.sha256 = "wrong".into();
	}
	scope.rows.insert(id(2), vec![corrupt]);
	let error = load(
		&mut scope,
		id(10),
		&[(1, id(1), 128000), (2, id(2), 128000)],
		&model,
	)
	.await
	.err()
	.unwrap();
	if length {
		assert!(
			matches!(error,Error::Invalid(ref message) if message=="run media input size changed")
		);
	} else {
		assert!(matches!(error,Error::Conflict(ref message) if message=="OBJECT_INTEGRITY"));
	}
	assert_eq!(scope.calls.last(), Some(&("attachments", id(2))));
}
#[rstest]
#[case::unsupported_type(true)]
#[case::bad_signature(false)]
#[tokio::test]
async fn media_type_and_signature_rejection_keep_the_domain_error(
	mut scope: Scope,
	model: ModelConfig,
	#[case] unsupported: bool,
) {
	let bytes = if unsupported {
		image().content
	} else {
		b"not an image".to_vec()
	};
	scope
		.rows
		.insert(id(1), vec![attachment("saved", "image/unsupported", bytes)]);
	if !unsupported {
		scope.rows.get_mut(&id(1)).unwrap()[0].media_type = "image/png".into();
	}
	let error = load(&mut scope, id(10), &[(1, id(1), 128000)], &model)
		.await
		.err()
		.unwrap();
	assert!(matches!(
		error,
		Error::Domain(aidash_domain::Error::Invalid(_))
	));
}
#[rstest]
#[tokio::test]
async fn first_message_budget_failure_returns_the_existing_context_error(
	mut scope: Scope,
	model: ModelConfig,
) {
	let error = load(&mut scope, id(10), &[(1, id(1), 0)], &model)
		.await
		.err()
		.unwrap();
	assert!(
		matches!(error,Error::Domain(aidash_domain::Error::Invalid(ref message)) if message.starts_with("model request exceeds context window:"))
	);
	assert_eq!(scope.calls, vec![("attachments", id(1))]);
}
#[rstest]
#[tokio::test]
async fn later_budget_failure_truncates_every_part_from_the_deferred_message(
	mut scope: Scope,
	model: ModelConfig,
) {
	scope.rows.insert(id(1), vec![image()]);
	scope.rows.insert(id(2), vec![audio()]);
	let output = load(
		&mut scope,
		id(10),
		&[(1, id(1), 128000), (2, id(2), 0)],
		&model,
	)
	.await
	.unwrap();
	assert_eq!(output.through_seq, Some(1));
	assert!(output.has_more);
	assert_eq!(
		parts(&output.parts),
		vec![
			json!({"text":"Run message 1 attachment: saved.png"}),
			json!({"image":"image/png","bytes":image().content})
		]
	);
}
#[rstest]
#[case::absent(false)]
#[case::expired(true)]
#[tokio::test]
async fn first_message_requires_a_current_approved_route(
	mut scope: Scope,
	mut model: ModelConfig,
	#[case] expired: bool,
) {
	if expired {
		model.media_routes[0].expires_at = Utc::now() - Duration::seconds(1);
	} else {
		model.media_routes.clear();
	}
	let error = load(&mut scope, id(10), &[(1, id(1), 128000)], &model)
		.await
		.err()
		.unwrap();
	assert!(matches!(error,Error::MediaRouteUnavailable(ref name) if name=="saved-model"));
}
#[rstest]
#[tokio::test]
async fn separate_format_routes_cannot_authorize_one_combined_media_batch(
	mut scope: Scope,
	mut model: ModelConfig,
) {
	scope.rows.insert(id(1), vec![image()]);
	scope.rows.insert(id(2), vec![audio()]);
	model.media_routes = vec![routes(vec!["image/png".into()]), routes(vec!["mp3".into()])];
	let output = load(
		&mut scope,
		id(10),
		&[(1, id(1), 128000), (2, id(2), 128000)],
		&model,
	)
	.await
	.unwrap();
	assert_eq!(output.through_seq, Some(1));
	assert!(output.has_more);
	assert_eq!(output.parts.len(), 2);
	assert!(matches!(&output.parts[1], ContentPart::Image { .. }));
}
#[rstest]
#[tokio::test]
async fn even_deferred_messages_keep_the_original_authorization_preflight(
	mut scope: Scope,
	model: ModelConfig,
) {
	scope.rows.insert(id(1), vec![image(); 8]);
	scope.rows.insert(id(2), vec![image()]);
	let output = authorized(
		&mut scope,
		id(10),
		&[(1, id(1), 128000), (2, id(2), 128000), (3, id(3), 128000)],
		&model,
	)
	.await
	.unwrap();
	assert_eq!(output.through_seq, Some(1));
	assert!(output.has_more);
	assert_eq!(
		scope.calls,
		vec![
			("authorize", id(1)),
			("authorize", id(2)),
			("authorize", id(3)),
			("attachments", id(1)),
			("attachments", id(2))
		]
	);
}
#[rstest]
#[tokio::test]
async fn attachment_read_faults_retain_their_identity_without_partial_output(
	mut scope: Scope,
	model: ModelConfig,
) {
	scope.fail = Some(id(2));
	let Error::Port(error) = load(
		&mut scope,
		id(10),
		&[(1, id(1), 128000), (2, id(2), 128000)],
		&model,
	)
	.await
	.err()
	.unwrap() else {
		panic!("expected adapter fault")
	};
	assert_eq!(
		error.downcast_ref::<std::io::Error>().unwrap().to_string(),
		"attachment fault"
	);
	assert_eq!(scope.calls.last(), Some(&("attachments", id(2))));
}
#[derive(Default)]
struct Journal {
	events: Vec<&'static str>,
	stall: bool,
	commit_failure: bool,
}
struct Repository {
	journal: Arc<Mutex<Journal>>,
}
struct Transaction {
	journal: Arc<Mutex<Journal>>,
	committed: bool,
}
impl Drop for Transaction {
	fn drop(&mut self) {
		self.journal.lock().unwrap().events.push(if self.committed {
			"release"
		} else {
			"rollback"
		});
	}
}
#[async_trait]
impl OperatorMediaRepository for Repository {
	fn node_id(&self) -> &str {
		"aidash://home"
	}
	async fn begin(&self) -> Result<Box<dyn MediaTransaction>> {
		self.journal.lock().unwrap().events.push("begin");
		Ok(Box::new(Transaction {
			journal: self.journal.clone(),
			committed: false,
		}))
	}
}
#[async_trait]
impl MediaAttachments for Transaction {
	async fn attachments(
		&mut self,
		workspace: Uuid,
		message: Uuid,
	) -> Result<Vec<MediaAttachment>> {
		assert_eq!(workspace, id(10));
		assert_eq!(message, id(1));
		let stall = {
			let mut journal = self.journal.lock().unwrap();
			journal.events.push("read");
			journal.stall
		};
		if stall {
			std::future::pending::<()>().await;
		}
		Ok(vec![image()])
	}
}
#[async_trait]
impl MediaTransaction for Transaction {
	async fn commit(mut self: Box<Self>) -> Result<()> {
		let mut journal = self.journal.lock().unwrap();
		journal.events.push("commit");
		if journal.commit_failure {
			return Err(Error::Port(Box::new(std::io::Error::other("commit fault"))));
		}
		self.committed = true;
		Ok(())
	}
}
#[fixture]
fn run() -> RunMetadata {
	RunMetadata {
		id: id(9),
		task_id: id(11),
		workspace_id: id(10),
		home_node: "aidash://home".into(),
		agent_id: "agent".into(),
		agent_version: "1".into(),
		phase: RunPhase::Ready,
		control: RunControl::Active,
		step: 0,
		revision: 1,
		observed_input_seq: 0,
		ledger_worker_ready: false,
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: Utc::now(),
	}
}
#[fixture]
fn repository() -> Repository {
	Repository {
		journal: Arc::new(Mutex::new(Journal::default())),
	}
}
#[rstest]
#[tokio::test]
async fn an_operator_cannot_open_a_media_transaction_for_a_foreign_run(
	repository: Repository,
	mut run: RunMetadata,
	model: ModelConfig,
) {
	run.home_node = "aidash://other".into();
	assert!(matches!(
		operator(&repository, &run, &[(1, id(1), 128000)], &model).await,
		Err(Error::Forbidden)
	));
	assert!(repository.journal.lock().unwrap().events.is_empty());
}
#[rstest]
#[tokio::test]
async fn an_operator_commits_before_returning_resolved_media(
	repository: Repository,
	run: RunMetadata,
	model: ModelConfig,
) {
	let output = operator(&repository, &run, &[(1, id(1), 128000)], &model)
		.await
		.unwrap();
	assert_eq!(output.through_seq, Some(1));
	assert_eq!(
		repository.journal.lock().unwrap().events,
		vec!["begin", "read", "commit", "release"]
	);
}
#[rstest]
#[tokio::test]
async fn an_operator_budget_error_rolls_back_without_publishing_a_batch(
	repository: Repository,
	run: RunMetadata,
	model: ModelConfig,
) {
	assert!(
		operator(&repository, &run, &[(1, id(1), 0)], &model)
			.await
			.is_err()
	);
	assert_eq!(
		repository.journal.lock().unwrap().events,
		vec!["begin", "read", "rollback"]
	);
}
#[rstest]
#[tokio::test]
async fn a_commit_error_preserves_its_identity_and_never_returns_media(
	repository: Repository,
	run: RunMetadata,
	model: ModelConfig,
) {
	repository.journal.lock().unwrap().commit_failure = true;
	let Error::Port(error) = operator(&repository, &run, &[(1, id(1), 128000)], &model)
		.await
		.err()
		.unwrap()
	else {
		panic!("expected commit fault")
	};
	assert_eq!(
		error.downcast_ref::<std::io::Error>().unwrap().to_string(),
		"commit fault"
	);
	assert_eq!(
		repository.journal.lock().unwrap().events,
		vec!["begin", "read", "commit", "rollback"]
	);
}
#[rstest]
#[tokio::test]
async fn cancelling_an_operator_read_releases_the_owned_transaction(
	repository: Repository,
	run: RunMetadata,
	model: ModelConfig,
) {
	repository.journal.lock().unwrap().stall = true;
	let messages = [(1, id(1), 128000)];
	let mut pending = Box::pin(operator(&repository, &run, &messages, &model));
	assert!(futures_util::poll!(pending.as_mut()).is_pending());
	drop(pending);
	assert_eq!(
		repository.journal.lock().unwrap().events,
		vec!["begin", "read", "rollback"]
	);
}

use crate::ports::execution::media::SelectedMediaScope;
use aidash_domain::media::{SelectedFile, Selection};

struct FileScope {
	files: Vec<SelectedFile>,
	content: BTreeMap<Uuid, Vec<u8>>,
	calls: Vec<(&'static str, Uuid)>,
	denied: bool,
	read_fault: Option<Uuid>,
}
#[fixture]
fn file_scope() -> FileScope {
	let records = [(id(1), image()), (id(2), audio())];
	FileScope {
		files: records
			.iter()
			.map(|(file_id, media)| SelectedFile {
				file_id: *file_id,
				path: format!("inputs/{}", media.filename),
				digest: media.sha256.clone(),
				size: media.content.len() as u64,
				media_type: media.media_type.clone(),
			})
			.collect(),
		content: records
			.into_iter()
			.map(|(file_id, media)| (file_id, media.content))
			.collect(),
		calls: vec![],
		denied: false,
		read_fault: None,
	}
}
fn selection(scope: &FileScope, file_id: Uuid) -> Selection {
	Selection {
		file_id,
		expected_digest: scope
			.files
			.iter()
			.find(|file| file.file_id == file_id)
			.unwrap()
			.digest
			.clone(),
	}
}
#[async_trait]
impl SelectedMediaScope for FileScope {
	async fn current_files(&mut self) -> Result<Vec<SelectedFile>> {
		self.calls.push(("snapshot", Uuid::nil()));
		if self.denied {
			Err(Error::Forbidden)
		} else {
			Ok(self.files.clone())
		}
	}
	async fn read(&mut self, file: Uuid) -> Result<Vec<u8>> {
		self.calls.push(("read", file));
		if self.read_fault == Some(file) {
			return Err(Error::Port(Box::new(std::io::Error::other(
				"working file fault",
			))));
		}
		Ok(self.content[&file].clone())
	}
}
#[rstest]
#[tokio::test]
async fn selected_files_preserve_input_order_duplicates_labels_and_bytes(
	mut file_scope: FileScope,
) {
	// Arrange: a repeated reference must remain repeated in the model input.
	let selections = [
		selection(&file_scope, id(2)),
		selection(&file_scope, id(1)),
		selection(&file_scope, id(2)),
	];
	// Act.
	let output = selected(&mut file_scope, &selections).await.unwrap();
	// Assert: one authorized snapshot precedes every ordered byte read.
	assert_eq!(
		file_scope.calls,
		vec![
			("snapshot", Uuid::nil()),
			("read", id(2)),
			("read", id(1)),
			("read", id(2))
		]
	);
	assert_eq!(
		parts(&output),
		vec![
			json!({"text":"Selected file: inputs/saved.mp3"}),
			json!({"audio":"mp3","bytes":audio().content}),
			json!({"text":"Selected file: inputs/saved.png"}),
			json!({"image":"image/png","bytes":image().content}),
			json!({"text":"Selected file: inputs/saved.mp3"}),
			json!({"audio":"mp3","bytes":audio().content}),
		]
	);
}
#[rstest]
#[tokio::test]
async fn selected_count_limit_precedes_authority_and_file_snapshot(mut file_scope: FileScope) {
	file_scope.denied = true;
	let selections = vec![selection(&file_scope, id(1)); 9];
	let error = selected(&mut file_scope, &selections).await.err().unwrap();
	assert!(
		matches!(error,Error::Invalid(ref message) if message == "model media input exceeds count limit")
	);
	assert!(file_scope.calls.is_empty());
}
#[rstest]
#[tokio::test]
async fn selected_empty_inputs_still_require_the_current_file_authority(mut file_scope: FileScope) {
	let output = selected(&mut file_scope, &[]).await.unwrap();
	assert!(output.is_empty());
	assert_eq!(file_scope.calls, vec![("snapshot", Uuid::nil())]);
}
#[rstest]
#[tokio::test]
async fn selected_denial_never_reads_file_bytes(mut file_scope: FileScope) {
	let selections = [selection(&file_scope, id(1))];
	file_scope.denied = true;
	assert!(matches!(
		selected(&mut file_scope, &selections).await,
		Err(Error::Forbidden)
	));
	assert_eq!(file_scope.calls, vec![("snapshot", Uuid::nil())]);
}
#[rstest]
#[case::first(false)]
#[case::after_valid_reference(true)]
#[tokio::test]
async fn selected_missing_file_keeps_order_and_the_original_not_found(
	mut file_scope: FileScope,
	#[case] after_valid: bool,
) {
	let mut selections = vec![];
	if after_valid {
		selections.push(selection(&file_scope, id(1)));
	}
	selections.push(Selection {
		file_id: id(99),
		expected_digest: "unavailable".into(),
	});
	let error = selected(&mut file_scope, &selections).await.err().unwrap();
	assert!(matches!(error,Error::NotFound(ref message) if message == "file unavailable"));
	let mut expected = vec![("snapshot", Uuid::nil())];
	if after_valid {
		expected.push(("read", id(1)));
	}
	assert_eq!(file_scope.calls, expected);
}
#[rstest]
#[case::first(false)]
#[case::after_valid_reference(true)]
#[tokio::test]
async fn selected_changed_digest_is_rejected_before_reading_that_file(
	mut file_scope: FileScope,
	#[case] after_valid: bool,
) {
	let mut selections = vec![];
	if after_valid {
		selections.push(selection(&file_scope, id(1)));
	}
	selections.push(Selection {
		file_id: id(2),
		expected_digest: "stale digest".into(),
	});
	let error = selected(&mut file_scope, &selections).await.err().unwrap();
	assert!(matches!(error,Error::Conflict(ref message) if message == "FILE_CHANGED"));
	let mut expected = vec![("snapshot", Uuid::nil())];
	if after_valid {
		expected.push(("read", id(1)));
	}
	assert_eq!(file_scope.calls, expected);
}
#[rstest]
#[case::first(false)]
#[case::aggregate(true)]
#[tokio::test]
async fn selected_byte_limit_precedes_the_excess_file_read(
	mut file_scope: FileScope,
	#[case] aggregate: bool,
) {
	let first = selection(&file_scope, id(1));
	let second = selection(&file_scope, id(2));
	file_scope.files[0].size = 8 * 1024 * 1024;
	let selections = if aggregate {
		vec![first, second]
	} else {
		file_scope.files[0].size += 1;
		vec![first]
	};
	let error = selected(&mut file_scope, &selections).await.err().unwrap();
	assert!(
		matches!(error,Error::Invalid(ref message) if message == "model media input exceeds byte limit")
	);
	let mut expected = vec![("snapshot", Uuid::nil())];
	if aggregate {
		expected.push(("read", id(1)));
	}
	assert_eq!(file_scope.calls, expected);
}
#[rstest]
#[tokio::test]
async fn selected_exact_byte_limit_is_inclusive(mut file_scope: FileScope) {
	let mut bytes = vec![0; 8 * 1024 * 1024];
	bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
	file_scope.files[0].size = bytes.len() as u64;
	file_scope.files[0].digest = format!("{:x}", Sha256::digest(&bytes));
	file_scope.content.insert(id(1), bytes.clone());
	let selections = [selection(&file_scope, id(1))];
	let output = selected(&mut file_scope, &selections).await.unwrap();
	assert!(
		matches!(&output[1],ContentPart::Image{media_type,bytes:actual} if media_type=="image/png" && actual==&bytes)
	);
	assert_eq!(
		file_scope.calls,
		vec![("snapshot", Uuid::nil()), ("read", id(1))]
	);
}
#[rstest]
#[tokio::test]
async fn selected_duplicate_metadata_uses_the_first_matching_saved_file(mut file_scope: FileScope) {
	let mut duplicate = file_scope.files[0].clone();
	duplicate.digest = "later duplicate digest".into();
	file_scope.files.push(duplicate);
	let selections = [Selection {
		file_id: id(1),
		expected_digest: "later duplicate digest".into(),
	}];
	assert!(
		matches!(selected(&mut file_scope,&selections).await,Err(Error::Conflict(ref message)) if message == "FILE_CHANGED")
	);
	assert_eq!(file_scope.calls, vec![("snapshot", Uuid::nil())]);
}
#[rstest]
#[tokio::test]
async fn selected_read_faults_keep_the_adapter_error_identity(mut file_scope: FileScope) {
	let selections = [selection(&file_scope, id(1)), selection(&file_scope, id(2))];
	file_scope.read_fault = Some(id(2));
	let Error::Port(error) = selected(&mut file_scope, &selections).await.err().unwrap() else {
		panic!("expected working file fault")
	};
	assert_eq!(
		error.downcast_ref::<std::io::Error>().unwrap().to_string(),
		"working file fault"
	);
	assert_eq!(
		file_scope.calls,
		vec![("snapshot", Uuid::nil()), ("read", id(1)), ("read", id(2))]
	);
}
#[rstest]
#[case::unsupported(true)]
#[case::invalid_signature(false)]
#[tokio::test]
async fn selected_media_rejection_keeps_the_domain_error(
	mut file_scope: FileScope,
	#[case] unsupported: bool,
) {
	let selections = [selection(&file_scope, id(1))];
	if unsupported {
		file_scope.files[0].media_type = "image/unsupported".into();
	} else {
		file_scope.content.insert(id(1), b"not an image".to_vec());
	}
	assert!(matches!(
		selected(&mut file_scope, &selections).await,
		Err(Error::Domain(aidash_domain::Error::Invalid(_)))
	));
	assert_eq!(
		file_scope.calls,
		vec![("snapshot", Uuid::nil()), ("read", id(1))]
	);
}
