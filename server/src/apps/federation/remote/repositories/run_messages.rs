//! Native ledger transactions and typed Home RPCs implement the shared message scope.
use crate::{
	domain::*,
	federation::{Federation, Home},
};
use aidash_application::{Result, ports::federation::run_messages::RunMessages};
use async_trait::async_trait;
use serde_json::{Value, json};
use uuid::Uuid;

pub(crate) struct Messages<'a> {
	pub(crate) federation: &'a Federation,
	pub(crate) run: RunMetadata,
	pub(crate) execution: Option<&'a Run>,
}
impl Messages<'_> {
	fn home(&self) -> Home {
		Home::for_delivery(self.federation.clone(), self.run.clone())
	}
}
#[async_trait]
impl RunMessages for Messages<'_> {
	fn node(&self) -> &str {
		&self.federation.config.node_id
	}
	fn run(&self) -> &RunMetadata {
		&self.run
	}
	async fn inputs(&self) -> Result<Vec<aidash_domain::run_input::RunInput>> {
		self.federation
			.store
			.run_inputs(self.run.id)
			.await
			.map_err(Into::into)
	}
	async fn has_media(&self, message: Uuid) -> Result<bool> {
		self.federation
			.store
			.run_message_has_media(&[message])
			.await
			.map_err(Into::into)
	}
	async fn sequence(&self, key: &str, content: &str) -> Result<i64> {
		self.federation
			.store
			.run_input_sequence(self.run.id, key, content)
			.await
			.map_err(Into::into)
	}
	async fn input_limit(&self) -> Result<usize> {
		let run = self.execution.ok_or_else(|| {
			aidash_application::Error::Invalid("delivery has no executable continuation".into())
		})?;
		self.federation
			.run_message_limit(run)
			.await
			.map_err(Into::into)
	}
	async fn accept(&self, sender: &str, content: &str, key: &str, limit: usize) -> Result<()> {
		self.federation
			.store
			.accept_run_message(self.run.id, sender, content, key, limit)
			.await
			.map_err(Into::into)
	}
	async fn import_and_accept(
		&self,
		history: &[(String, Message)],
		sender: &str,
		content: &str,
		key: &str,
		limit: usize,
	) -> Result<()> {
		self.federation
			.store
			.import_remote_run_messages_and_accept(
				self.run.id,
				history,
				sender,
				content,
				key,
				limit,
			)
			.await
			.map_err(Into::into)
	}
	async fn import_history(&self, history: &[(String, Message)], limit: usize) -> Result<()> {
		self.federation
			.store
			.import_remote_run_messages(self.run.id, history, limit)
			.await
			.map_err(Into::into)
	}
	async fn bind(&self, key: &str, message: Uuid) -> Result<()> {
		self.federation
			.store
			.bind_run_input_message(self.run.id, key, message)
			.await
			.map_err(Into::into)
	}
	async fn reserve(&self, key: &str, content: &str) -> Result<bool> {
		Ok(self
			.home()
			.optional_command::<Value>(
				"run_message_reserve",
				json!({"run_id":self.run.id,"key":key,"content":content}),
			)
			.await?
			.is_some())
	}
	async fn promote(&self, key: &str, content: &str, sequence: i64) -> Result<bool> {
		Ok(self
			.home()
			.optional_command::<Value>(
				"run_message_commit",
				json!({"run_id":self.run.id,"key":key,"content":content,"input_seq":sequence}),
			)
			.await?
			.is_some())
	}
	async fn release(&self, keys: &[String]) -> Result<()> {
		self.home()
			.optional_command::<Value>(
				"run_message_release",
				json!({"run_id":self.run.id,"keys":keys}),
			)
			.await?;
		Ok(())
	}
	async fn acknowledge(&self, keys: &[String]) -> Result<()> {
		self.home()
			.optional_command::<Value>("run_message_ack", json!({"run_id":self.run.id,"keys":keys}))
			.await?;
		Ok(())
	}
	async fn delivery(&self, key: &str, content: &str) -> Result<Option<Message>> {
		if self.local() {
			return self
				.federation
				.store
				.message_record(self.run.workspace_id, "human", content, Some(key))
				.await
				.map(Some)
				.map_err(Into::into);
		}
		self.home()
			.optional_command(
				"run_message_delivery",
				json!({"run_id":self.run.id,"key":key,"content":content}),
			)
			.await
			.map_err(Into::into)
	}
	async fn legacy_message(&self, key: &str, content: &str) -> Result<()> {
		self.home()
			.command::<Value>("human_message", json!({"key":key,"content":content}))
			.await?;
		Ok(())
	}
	async fn snapshot_page(&self, after: Option<Uuid>) -> Result<SnapshotPage> {
		self.home()
			.command(
				"snapshot_page",
				json!({"collection":"messages","after":after}),
			)
			.await
			.map_err(Into::into)
	}
	async fn history_page(&self, offset: usize) -> Result<Option<Vec<Message>>> {
		self.home()
			.optional_command(
				"run_message_history",
				json!({"run_id":self.run.id,"offset":offset}),
			)
			.await
			.map_err(Into::into)
	}
	async fn capability(&self) -> Result<Option<Value>> {
		self.home()
			.optional_command("run_message_delivery_capability", json!({}))
			.await
			.map_err(Into::into)
	}
}
