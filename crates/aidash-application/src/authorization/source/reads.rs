//! Current viewer checks precede original credential authority, without importing producer caches.
use crate::{Error, Result, ports::authorization::source::reads::SourceReadScope};
use aidash_domain::{federation::execution::Inspection, semantic::remote::Binding};
use uuid::Uuid;
pub async fn visible<S: SourceReadScope + ?Sized>(
	scope: &mut S,
	node: &str,
	id: Uuid,
	admission: Uuid,
) -> Result<bool> {
	let Some(grant) = scope.reader_grant(node, id).await? else {
		return Ok(false);
	};
	let Some(bound) = scope.read_binding(id).await? else {
		return Ok(false);
	};
	if bound.admission_id != admission || bound.task_id != grant.task_id {
		return Ok(false);
	}
	let protocol = scope.protocol_version().to_owned();
	super::grants::require_peer(scope, node, &protocol).await?;
	let task = scope.read_task(grant.task_id).await?;
	if task.revision != bound.task_revision {
		return Ok(false);
	}
	if !scope.grant_reads(id).await? {
		return Ok(false);
	}
	{
		// The producer port owns restoration while this future is pending, rejected or cancelled.
		let mut producer = scope.producer(&grant).await?;
		let inspection: Inspection = serde_json::from_value(grant.inspection)?;
		super::authorize(producer.as_mut(), &task, node, &inspection).await?;
		producer.producer_semantic_sources(id).await?;
	}
	scope.record_admission(node, id, admission)?;
	Ok(true)
}
pub async fn output_visible<S: SourceReadScope + ?Sized>(scope: &mut S, id: Uuid) -> Result<bool> {
	let Some(_visit) = scope.output_visit(id) else {
		return Ok(true);
	};
	let Some((node, semantic)) = scope.output_record(id).await? else {
		return Ok(false);
	};
	if serde_json::from_value::<Binding>(semantic)?.disabled() {
		return scope.grant_reads(id).await;
	}
	let Some(bound) = scope.read_binding(id).await? else {
		return Ok(false);
	};
	let coordinator = !scope.collecting_dependencies();
	if coordinator {
		scope.start_dependencies();
	}
	let mut result = Box::pin(visible(scope, &node, id, bound.admission_id)).await;
	if coordinator {
		let pending = scope.take_dependencies();
		if matches!(result, Ok(true)) {
			result = scope.verify_dependencies(pending).await;
		}
	}
	match result {
		Err(
			Error::Forbidden | Error::Unauthorized | Error::NotFound(_) | Error::RemoteSemantic(_),
		) => Ok(false),
		other => other,
	}
}
