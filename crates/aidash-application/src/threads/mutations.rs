//! Creation and mutations share authorization with worker and conversation callers.
use crate::{Result, ports::threads::mutations::ThreadMutations};
use aidash_domain::{NewTask, Task, Thread, nodes::NodeName};
use serde_json::json;
use uuid::Uuid;
pub async fn create(
	scope: &mut dyn ThreadMutations,
	id: Uuid,
	title: &str,
	node: Option<&NodeName>,
) -> Result<Thread> {
	let resource = scope.resource(
		"thread",
		id,
		json!({"owner":scope.identity().1,"thread_id":id}),
	);
	scope.require(&resource, "thread.create").await?;
	let node = scope.use_node(node).await?;
	let thread = scope.insert_thread(id, &node, title).await?;
	scope.record_owner(id).await?;
	Ok(thread)
}
/// Rename a Thread; its title is the only attribute that changes.
pub async fn update(
	scope: &mut dyn ThreadMutations,
	id: Uuid,
	revision: i64,
	title: &str,
) -> Result<Thread> {
	scope.require_thread(id, "thread.read").await?;
	scope.require_thread(id, "thread.update").await?;
	scope.update_title(id, revision, title).await
}
pub async fn create_task(
	scope: &mut dyn ThreadMutations,
	id: Uuid,
	input: &NewTask,
	key: Option<&str>,
) -> Result<Task> {
	scope.require_thread(id, "thread.read").await?;
	scope.require_thread(id, "task.create").await?;
	let key = key.map(|key| {
		format!(
			"subject:{}",
			aidash_domain::registry::rules::digest(&json!([
				scope.identity().0,
				scope.identity().1,
				id,
				key
			]))
		)
	});
	scope.related_tasks(id, input).await?;
	let subject = scope.identity().1.to_owned();
	let task = scope
		.insert_task(id, input, &subject, key.as_deref())
		.await?;
	let resource = scope.task_resource(&task).await?;
	scope.require(&resource, "task.read").await?;
	Ok(task)
}
pub async fn message(
	scope: &mut dyn ThreadMutations,
	id: Uuid,
	content: &str,
	nonce: Uuid,
) -> Result<()> {
	let key = format!(
		"thread-subject:{}:{}:{id}:{nonce}",
		scope.identity().0,
		scope.identity().1
	);
	scope.require_thread(id, "message.create").await?;
	let sender = scope.identity().1.to_owned();
	scope.insert_message(id, &sender, content, &key).await
}
#[cfg(test)]
mod tests;
