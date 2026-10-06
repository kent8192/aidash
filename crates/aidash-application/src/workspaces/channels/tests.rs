use super::*;
use crate::ports::channels::{AttachmentLink, HistoryRow, MessageContext};
use aidash_domain::Message;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rstest::rstest;
use std::collections::{HashMap, HashSet, VecDeque};

fn workspace() -> Uuid {
	Uuid::from_u128(1)
}
fn message(id: u128) -> Message {
	Message {
		id: Uuid::from_u128(id),
		workspace_id: workspace(),
		sender: "fixture".into(),
		content: "content".into(),
		idempotency_key: None,
		created_at: DateTime::from_timestamp(id as i64, 0).unwrap(),
	}
}
fn thread_record() -> ChannelThread {
	ChannelThread {
		id: Uuid::from_u128(9),
		workspace_id: workspace(),
		root_message_id: Uuid::from_u128(2),
		created_by: "fixture".into(),
		created_at: Utc::now(),
	}
}
fn metadata(id: u128) -> ChannelAttachment {
	ChannelAttachment {
		id: Uuid::from_u128(id),
		filename: format!("{id}.txt"),
		media_type: "text/plain".into(),
		size_bytes: 1,
	}
}
fn attachment(key: Uuid) -> AttachmentState {
	AttachmentState {
		id: Uuid::from_u128(10),
		workspace_id: workspace(),
		uploaded_by: "principal".into(),
		idempotency_key: key,
		filename: "10.txt".into(),
		media_type: "text/plain".into(),
		sha256: format!("{:x}", Sha256::digest(b"x")),
		size_bytes: 1,
		content: b"x".to_vec(),
		message_id: Some(Uuid::from_u128(2)),
	}
}
#[derive(Default)]
struct Fixture {
	calls: Vec<String>,
	denied: Option<&'static str>,
	context: Option<MessageContext>,
	links: Vec<AttachmentLink>,
	bind_order: Vec<(Uuid, i32)>,
	inserted: bool,
	reply: Option<Uuid>,
	history: VecDeque<Vec<HistoryRow>>,
	hidden: HashSet<Uuid>,
	positions: Vec<(Uuid, i32)>,
	stored: Option<AttachmentState>,
	key: Option<String>,
}
impl Fixture {
	fn record(&mut self, op: &'static str) -> Result<()> {
		self.calls.push(op.into());
		if self.denied == Some(op) {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
}
#[async_trait]
impl ChannelScope for Fixture {
	fn sender(&self) -> String {
		"fixture".into()
	}
	fn principal(&self) -> String {
		"principal".into()
	}
	async fn message(&mut self, _: Uuid, id: Uuid) -> Result<Message> {
		self.record("message.read")?;
		Ok(message(id.as_u128()))
	}
	async fn visible(&mut self, message: &Message) -> Result<bool> {
		self.record("message.visible")?;
		Ok(!self.hidden.contains(&message.id))
	}
	async fn thread_visible(&mut self, _: Uuid) -> Result<()> {
		self.record("thread.visible")
	}
	async fn thread(&mut self, _: Uuid, _: Uuid) -> Result<Option<ChannelThread>> {
		self.record("thread.read")?;
		Ok(Some(thread_record()))
	}
	async fn reply_thread(&mut self, _: Uuid) -> Result<Option<Uuid>> {
		self.record("reply.read")?;
		Ok(self.reply)
	}
	async fn insert_thread(&mut self, _: Uuid, _: Uuid, _: &str) -> Result<Option<ChannelThread>> {
		self.record("thread.insert")?;
		Ok(self.inserted.then(thread_record))
	}
	async fn existing_thread(&mut self, _: Uuid, _: Uuid) -> Result<ChannelThread> {
		self.record("thread.existing")?;
		Ok(thread_record())
	}
	async fn thread_event(&mut self, _: Uuid, _: Uuid, _: Uuid) -> Result<()> {
		self.record("thread.event")
	}
	async fn append_message(&mut self, _: Uuid, _: &str, _: &str, key: &str) -> Result<Message> {
		self.record("message.insert")?;
		self.key = Some(key.into());
		Ok(message(2))
	}
	async fn record_context(
		&mut self,
		_: Uuid,
		_: Uuid,
		thread: Option<Uuid>,
		digest: &str,
	) -> Result<MessageContext> {
		self.record("message.context")?;
		Ok(self.context.clone().unwrap_or(MessageContext {
			thread_id: thread,
			attachment_digest: digest.into(),
		}))
	}
	async fn root_thread(&mut self, _: Uuid) -> Result<Option<Uuid>> {
		self.record("root.read")?;
		Ok(None)
	}
	async fn legacy_attachment_positions(&mut self, _: Uuid, _: Uuid) -> Result<Vec<(Uuid, i32)>> {
		self.record("attachment.legacy")?;
		Ok(self.positions.clone())
	}
	async fn attachment_links(
		&mut self,
		_: Uuid,
		_: &[Uuid],
		_: &str,
	) -> Result<Vec<AttachmentLink>> {
		self.record("attachment.lock")?;
		Ok(self.links.clone())
	}
	async fn bind_attachment(
		&mut self,
		_: Uuid,
		_: Uuid,
		id: Uuid,
		_: &str,
		position: i32,
	) -> Result<()> {
		self.record("attachment.bind")?;
		self.bind_order.push((id, position));
		Ok(())
	}
	async fn insert_attachment(
		&mut self,
		_: Uuid,
		_: &str,
		_: &AttachmentUpload<'_>,
		_: &str,
		_: &[u8],
	) -> Result<Option<AttachmentState>> {
		self.record("attachment.insert")?;
		Ok(None)
	}
	async fn existing_attachment(&mut self, _: Uuid, _: &str, _: Uuid) -> Result<AttachmentState> {
		self.record("attachment.existing")?;
		Ok(self.stored.clone().unwrap())
	}
	async fn attachment(&mut self, _: Uuid, _: Uuid) -> Result<Option<AttachmentState>> {
		self.record("attachment.read")?;
		Ok(self.stored.clone())
	}
	async fn history_rows(
		&mut self,
		_: Uuid,
		_: Option<&ChannelThread>,
		_: Option<(DateTime<Utc>, Uuid)>,
	) -> Result<Vec<HistoryRow>> {
		self.record("history.read")?;
		Ok(self.history.pop_front().unwrap_or_default())
	}
	async fn message_attachments(
		&mut self,
		_: Uuid,
		ids: &[Uuid],
	) -> Result<HashMap<Uuid, Vec<ChannelAttachment>>> {
		self.record("history.attachments")?;
		Ok(ids.iter().map(|id| (*id, vec![metadata(10)])).collect())
	}
}
fn submission<'a>(ids: &'a [Uuid]) -> MessageSubmission<'a> {
	MessageSubmission {
		content: "content",
		thread_id: None,
		idempotency_key: Uuid::from_u128(100),
		attachment_ids: ids,
	}
}

#[rstest]
#[tokio::test]
async fn denied_root_visibility_prevents_thread_and_event_creation() {
	let mut scope = Fixture {
		denied: Some("message.read"),
		inserted: true,
		..Default::default()
	};
	let result = create_thread(&mut scope, workspace(), Uuid::from_u128(2)).await;
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(scope.calls, vec!["message.read"]);
}
#[rstest]
#[case(true,vec!["message.read","reply.read","thread.insert","thread.event"])]
#[case(false,vec!["message.read","reply.read","thread.insert","thread.existing","thread.visible"])]
#[tokio::test]
async fn thread_replays_do_not_duplicate_events(
	#[case] inserted: bool,
	#[case] expected: Vec<&str>,
) {
	let mut scope = Fixture {
		inserted,
		..Default::default()
	};
	assert_eq!(
		create_thread(&mut scope, workspace(), Uuid::from_u128(2))
			.await
			.unwrap()
			.id,
		thread_record().id
	);
	assert_eq!(scope.calls, expected);
}
#[rstest]
#[tokio::test]
async fn thread_replay_conflict_stops_before_attachment_binding() {
	let mut scope = Fixture {
		context: Some(MessageContext {
			thread_id: Some(Uuid::from_u128(9)),
			attachment_digest: String::new(),
		}),
		..Default::default()
	};
	let result = post(&mut scope, workspace(), submission(&[Uuid::from_u128(10)])).await;
	assert!(
		matches!(result,Err(Error::Conflict(ref value)) if value=="message idempotency key reused for a different thread")
	);
	assert_eq!(scope.calls, vec!["message.insert", "message.context"]);
	assert_eq!(
		scope.key.unwrap(),
		format!("channel:{}:principal:{}", workspace(), Uuid::from_u128(100))
	);
}
#[rstest]
#[tokio::test]
async fn attachment_order_is_returned_and_bound_in_request_order() {
	let ids = [Uuid::from_u128(11), Uuid::from_u128(10)];
	let mut scope = Fixture {
		links: vec![
			AttachmentLink {
				attachment: metadata(10),
				message_id: None,
			},
			AttachmentLink {
				attachment: metadata(11),
				message_id: None,
			},
		],
		..Default::default()
	};
	let result = post(&mut scope, workspace(), submission(&ids))
		.await
		.unwrap();
	assert_eq!(
		result.attachments.iter().map(|x| x.id).collect::<Vec<_>>(),
		ids
	);
	assert_eq!(scope.bind_order, vec![(ids[0], 0), (ids[1], 1)]);
	assert_eq!(
		scope.calls,
		vec![
			"message.insert",
			"message.context",
			"attachment.lock",
			"attachment.bind",
			"attachment.bind",
			"root.read"
		]
	);
}
#[rstest]
#[tokio::test]
async fn already_linked_attachment_stops_all_binding() {
	let mut scope = Fixture {
		links: vec![AttachmentLink {
			attachment: metadata(10),
			message_id: Some(Uuid::from_u128(99)),
		}],
		..Default::default()
	};
	let result = post(&mut scope, workspace(), submission(&[Uuid::from_u128(10)])).await;
	assert!(
		matches!(result,Err(Error::Conflict(ref value)) if value=="attachment is already linked to another message")
	);
	assert!(scope.bind_order.is_empty());
	assert_eq!(
		scope.calls,
		vec!["message.insert", "message.context", "attachment.lock"]
	);
}
#[rstest]
#[tokio::test]
async fn upload_replay_compares_metadata_and_content_before_returning() {
	let key = Uuid::from_u128(100);
	let mut scope = Fixture {
		stored: Some(attachment(key)),
		..Default::default()
	};
	let result = upload(
		&mut scope,
		workspace(),
		AttachmentUpload {
			filename: "10.txt",
			media_type: "text/plain",
			idempotency_key: key,
		},
		b"changed",
	)
	.await;
	assert!(
		matches!(result,Err(Error::Conflict(ref value)) if value=="attachment idempotency key was reused with different content or metadata")
	);
	assert_eq!(
		scope.calls,
		vec!["attachment.insert", "attachment.existing"]
	);
}
#[rstest]
#[tokio::test]
async fn download_checks_current_message_visibility_before_returning_content() {
	let mut scope = Fixture {
		stored: Some(attachment(Uuid::from_u128(100))),
		denied: Some("message.read"),
		..Default::default()
	};
	let result = download(&mut scope, workspace(), Uuid::from_u128(10)).await;
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(scope.calls, vec!["attachment.read", "message.read"]);
}
#[rstest]
#[tokio::test]
async fn history_cursor_uses_only_visible_messages_and_pages_are_chronological() {
	let rows = (2..=6)
		.rev()
		.map(|id| HistoryRow {
			message: message(id),
			reply_thread_id: None,
			root_thread_id: None,
		})
		.collect();
	let mut scope = Fixture {
		history: VecDeque::from([rows]),
		hidden: HashSet::from([Uuid::from_u128(6), Uuid::from_u128(4)]),
		..Default::default()
	};
	let page = history(
		&mut scope,
		workspace(),
		HistoryRequest {
			thread_id: None,
			before: None,
			limit: 2,
		},
	)
	.await
	.unwrap();
	assert_eq!(
		page.messages
			.iter()
			.map(|x| x.message.id)
			.collect::<Vec<_>>(),
		vec![Uuid::from_u128(3), Uuid::from_u128(5)]
	);
	assert_eq!(page.next_before, Some(Uuid::from_u128(3)));
	assert_eq!(page.messages[0].attachments[0].id, Uuid::from_u128(10));
}
#[rstest]
#[tokio::test]
async fn foreign_thread_cursor_is_rejected_before_scanning_history() {
	let mut scope = Fixture {
		reply: Some(Uuid::from_u128(9)),
		..Default::default()
	};
	let result = history(
		&mut scope,
		workspace(),
		HistoryRequest {
			thread_id: None,
			before: Some(Uuid::from_u128(2)),
			limit: 2,
		},
	)
	.await;
	assert!(matches!(result,Err(Error::NotFound(ref value)) if value=="message unavailable"));
	assert_eq!(scope.calls, vec!["message.read", "reply.read"]);
}
