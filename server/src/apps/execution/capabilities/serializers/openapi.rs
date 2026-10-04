//! Native typed contracts for core capabilities.
use super::super::views;
use crate::{Result, config::openapi::Contracts};
use reinhardt::rest::openapi::OpenApiSchema;
pub(crate) fn register(contracts: &mut Contracts, document: &mut OpenApiSchema) -> Result<()> {
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::management::ConfiguredAgent>(
			document,
			views::management::configure_agent,
			200,
			"application/json",
		)?;
	contracts
		.request::<_, crate::apps::execution::capabilities::serializers::configuration::Configure>(
			document,
			views::management::configure_agent,
		)?;
	contracts.path(document, views::management::configure_agent, &["String"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::contracts::AreaPage>(
			document,
			views::management::areas,
			200,
			"application/json",
		)?;
	contracts.query::<_, crate::apps::execution::capabilities::serializers::endpoints::AreaQuery>(
		document,
		views::management::areas,
	)?;
	contracts.response::<_, crate::apps::execution::capabilities::serializers::contracts::Area>(
		document,
		views::management::area_for_run,
		200,
		"application/json",
	)?;
	contracts.path(document, views::management::area_for_run, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::contracts::Envelope>(
			document,
			views::management::file_read,
			200,
			"application/json",
		)?;
	contracts
		.request::<_, crate::apps::execution::capabilities::serializers::contracts::FileRead>(
			document,
			views::management::file_read,
		)?;
	contracts.path(document, views::management::file_read, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::contracts::Envelope>(
			document,
			views::management::file_search,
			200,
			"application/json",
		)?;
	contracts
		.request::<_, crate::apps::execution::capabilities::serializers::contracts::FileSearch>(
			document,
			views::management::file_search,
		)?;
	contracts.path(document, views::management::file_search, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::contracts::OperationResult>(
			document,
			views::management::shell,
			200,
			"application/json",
		)?;
	contracts.request::<_, crate::apps::execution::capabilities::serializers::contracts::Shell>(
		document,
		views::management::shell,
	)?;
	contracts.path(document, views::management::shell, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::contracts::Envelope>(
			document,
			views::management::apply_patch,
			200,
			"application/json",
		)?;
	contracts.request::<_, crate::apps::execution::capabilities::serializers::contracts::Patch>(
		document,
		views::management::apply_patch,
	)?;
	contracts.path(document, views::management::apply_patch, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::contracts::OperationResult>(
			document,
			views::management::shell_poll,
			200,
			"application/json",
		)?;
	contracts
		.request::<_, crate::apps::execution::capabilities::serializers::contracts::OperationInput>(
			document,
			views::management::shell_poll,
		)?;
	contracts.path(document, views::management::shell_poll, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::contracts::OperationResult>(
			document,
			views::management::shell_cancel,
			200,
			"application/json",
		)?;
	contracts
		.request::<_, crate::apps::execution::capabilities::serializers::contracts::OperationInput>(
			document,
			views::management::shell_cancel,
		)?;
	contracts.path(document, views::management::shell_cancel, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::management::MaterializedFile>(
			document,
			views::management::materialize,
			200,
			"application/json",
		)?;
	contracts
		.request::<_, crate::apps::execution::capabilities::serializers::contracts::Materialize>(
			document,
			views::management::materialize,
		)?;
	contracts.path(document, views::management::materialize, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::contracts::SessionStatus>(
			document,
			views::management::session,
			200,
			"application/json",
		)?;
	contracts.path(document, views::management::session, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::contracts::SessionStatus>(
			document,
			views::management::enqueue,
			200,
			"application/json",
		)?;
	contracts.request::<_, crate::apps::execution::capabilities::serializers::contracts::Enqueue>(
		document,
		views::management::enqueue,
	)?;
	contracts.path(document, views::management::enqueue, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::contracts::Steered>(
			document,
			views::management::steer,
			200,
			"application/json",
		)?;
	contracts.request::<_, crate::apps::execution::capabilities::serializers::contracts::Steer>(
		document,
		views::management::steer,
	)?;
	contracts.path(document, views::management::steer, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::skills::SkillAttachment>(
			document,
			views::management::skill_import,
			200,
			"application/json",
		)?;
	contracts.request::<_, crate::skill_import::ImportRequest>(
		document,
		views::management::skill_import,
	)?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::contracts::Envelope>(
			document,
			views::management::skill_list,
			200,
			"application/json",
		)?;
	contracts.request::<_, crate::apps::execution::capabilities::serializers::skills::SkillList>(
		document,
		views::management::skill_list,
	)?;
	contracts.path(document, views::management::skill_list, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::contracts::Envelope>(
			document,
			views::management::skill_load,
			200,
			"application/json",
		)?;
	contracts.request::<_, crate::apps::execution::capabilities::serializers::skills::SkillLoad>(
		document,
		views::management::skill_load,
	)?;
	contracts.path(document, views::management::skill_load, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::contracts::Envelope>(
			document,
			views::management::skill_read,
			200,
			"application/json",
		)?;
	contracts.request::<_, crate::apps::execution::capabilities::serializers::skills::SkillRead>(
		document,
		views::management::skill_read,
	)?;
	contracts.path(document, views::management::skill_read, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::contracts::Envelope>(
			document,
			views::management::file_share,
			200,
			"application/json",
		)?;
	contracts.request::<_, crate::apps::execution::capabilities::serializers::sharing::Share>(
		document,
		views::management::file_share,
	)?;
	contracts.path(document, views::management::file_share, &["Uuid"])?;
	contracts.response::<_, crate::apps::execution::capabilities::serializers::contracts::Area>(
		document,
		views::management::thread_run,
		200,
		"application/json",
	)?;
	contracts.request::<_, crate::apps::execution::capabilities::serializers::contracts::Enqueue>(
		document,
		views::management::thread_run,
	)?;
	contracts.path(
		document,
		views::management::thread_run,
		&["Uuid", "Uuid", "String"],
	)?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::contracts::Envelope>(
			document,
			views::management::outbound_get,
			200,
			"application/json",
		)?;
	contracts
		.request::<_, crate::apps::execution::capabilities::serializers::approvals::Outbound>(
			document,
			views::management::outbound_get,
		)?;
	contracts.path(document, views::management::outbound_get, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::endpoints::OutboundPage>(
			document,
			views::management::outbound_history,
			200,
			"application/json",
		)?;
	contracts.path(document, views::management::outbound_history, &["Uuid"])?;
	contracts.query::<_, crate::apps::execution::capabilities::serializers::endpoints::Page>(
		document,
		views::management::outbound_history,
	)?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::management::ApprovalPage>(
			document,
			views::management::approval_list,
			200,
			"application/json",
		)?;
	contracts.query::<_, crate::apps::execution::capabilities::serializers::endpoints::Page>(
		document,
		views::management::approval_list,
	)?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::management::ApprovalDecided>(
			document,
			views::management::approval_decide,
			200,
			"application/json",
		)?;
	contracts
		.request::<_, crate::apps::execution::capabilities::serializers::approvals::ApprovalDecision>(
			document,
			views::management::approval_decide,
		)?;
	contracts.path(document, views::management::approval_decide, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::management::Revoked>(
			document,
			views::management::approval_revoke,
			200,
			"application/json",
		)?;
	contracts.request::<_, crate::apps::execution::capabilities::serializers::approvals::Revoke>(
		document,
		views::management::approval_revoke,
	)?;
	contracts.path(document, views::management::approval_revoke, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::management::Revoked>(
			document,
			views::management::grant_revoke,
			200,
			"application/json",
		)?;
	contracts.request::<_, crate::apps::execution::capabilities::serializers::approvals::Revoke>(
		document,
		views::management::grant_revoke,
	)?;
	contracts.path(document, views::management::grant_revoke, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::contracts::Envelope>(
			document,
			views::management::python,
			200,
			"application/json",
		)?;
	contracts.request::<_, crate::apps::execution::capabilities::serializers::python::Python>(
		document,
		views::management::python,
	)?;
	contracts.path(document, views::management::python, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::contracts::Envelope>(
			document,
			views::management::python_install,
			200,
			"application/json",
		)?;
	contracts.request::<_, crate::apps::execution::capabilities::serializers::packages::Install>(
		document,
		views::management::python_install,
	)?;
	contracts.path(document, views::management::python_install, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::contracts::Envelope>(
			document,
			views::management::python_poll,
			200,
			"application/json",
		)?;
	contracts
		.request::<_, crate::apps::execution::capabilities::serializers::contracts::OperationInput>(
			document,
			views::management::python_poll,
		)?;
	contracts.path(document, views::management::python_poll, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::contracts::Envelope>(
			document,
			views::management::python_cancel,
			200,
			"application/json",
		)?;
	contracts
		.request::<_, crate::apps::execution::capabilities::serializers::contracts::OperationInput>(
			document,
			views::management::python_cancel,
		)?;
	contracts.path(document, views::management::python_cancel, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::references::Reference>(
			document,
			views::management::reference_upload,
			200,
			"application/json",
		)?;
	contracts.request::<_, crate::apps::execution::capabilities::serializers::references::Upload>(
		document,
		views::management::reference_upload,
	)?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::references::Reference>(
			document,
			views::management::reference_chunk,
			200,
			"application/json",
		)?;
	contracts.request::<_, crate::apps::execution::capabilities::serializers::transfer::Chunk>(
		document,
		views::management::reference_chunk,
	)?;
	contracts.path(document, views::management::reference_chunk, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::references::Reference>(
			document,
			views::management::reference_commit,
			200,
			"application/json",
		)?;
	contracts.path(document, views::management::reference_commit, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::references::Reference>(
			document,
			views::management::reference_inspect,
			200,
			"application/json",
		)?;
	contracts.path(document, views::management::reference_inspect, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::management::ReferenceRevoked>(
			document,
			views::management::reference_revoke,
			200,
			"application/json",
		)?;
	contracts
		.request::<_, crate::apps::execution::capabilities::serializers::endpoints::ExpectedRevision>(
			document,
			views::management::reference_revoke,
		)?;
	contracts.path(document, views::management::reference_revoke, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::endpoints::DownloadChunk>(
			document,
			views::management::reference_download,
			200,
			"application/json",
		)?;
	contracts.path(document, views::management::reference_download, &["Uuid"])?;
	contracts.query::<_, crate::apps::execution::capabilities::serializers::endpoints::Offset>(
		document,
		views::management::reference_download,
	)?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::endpoints::DownloadChunk>(
			document,
			views::management::file_download,
			200,
			"application/json",
		)?;
	contracts.path(
		document,
		views::management::file_download,
		&["Uuid", "Uuid"],
	)?;
	contracts.query::<_, crate::apps::execution::capabilities::serializers::endpoints::Offset>(
		document,
		views::management::file_download,
	)?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::cleanup::ManagementPage>(
			document,
			views::management::managed_areas,
			200,
			"application/json",
		)?;
	contracts.query::<_, crate::apps::execution::capabilities::serializers::endpoints::Page>(
		document,
		views::management::managed_areas,
	)?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::cleanup::CleanupResult>(
			document,
			views::management::cleanup,
			200,
			"application/json",
		)?;
	contracts.request::<_, crate::apps::execution::capabilities::serializers::cleanup::Cleanup>(
		document,
		views::management::cleanup,
	)?;
	contracts.path(document, views::management::cleanup, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::cleanup::CleanupResult>(
			document,
			views::management::cleanup_status,
			200,
			"application/json",
		)?;
	contracts.path(document, views::management::cleanup_status, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::cleanup::CleanupResult>(
			document,
			views::management::cleanup_reconcile,
			200,
			"application/json",
		)?;
	contracts.path(document, views::management::cleanup_reconcile, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::management::DeletionConfirmation>(
			document,
			views::management::deletion_confirmation,
			200,
			"application/json",
		)?;
	contracts
		.request::<_, crate::apps::execution::capabilities::serializers::endpoints::ExpectedRevision>(
			document,
			views::management::deletion_confirmation,
		)?;
	contracts.path(
		document,
		views::management::deletion_confirmation,
		&["Uuid"],
	)?;
	contracts.response::<_, crate::apps::execution::capabilities::serializers::contracts::Area>(
		document,
		views::management::restore,
		200,
		"application/json",
	)?;
	contracts.request::<_, crate::apps::execution::capabilities::serializers::cleanup::Restore>(
		document,
		views::management::restore,
	)?;
	contracts.path(document, views::management::restore, &["Uuid"])?;
	contracts.response::<_, serde_json::Value>(
		document,
		views::management::restore_new_thread,
		200,
		"application/json",
	)?;
	contracts
		.request::<_, crate::apps::execution::capabilities::serializers::cleanup::RestoreNewThread>(
			document,
			views::management::restore_new_thread,
		)?;
	contracts.path(
		document,
		views::management::restore_new_thread,
		&["Uuid", "Uuid"],
	)?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::management::TransferStatus>(
			document,
			views::management::transfer_status,
			200,
			"application/json",
		)?;
	contracts.path(document, views::management::transfer_status, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::management::TransferPage>(
			document,
			views::management::transfer_history,
			200,
			"application/json",
		)?;
	contracts.path(document, views::management::transfer_history, &["Uuid"])?;
	contracts.query::<_, crate::apps::execution::capabilities::serializers::endpoints::Page>(
		document,
		views::management::transfer_history,
	)?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::management::TransferStatus>(
			document,
			views::management::transfer_reconcile,
			200,
			"application/json",
		)?;
	contracts.path(document, views::management::transfer_reconcile, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::management::RecipientPage>(
			document,
			views::management::transfer_recipients,
			200,
			"application/json",
		)?;
	contracts
		.query::<_, crate::apps::execution::capabilities::serializers::endpoints::RecipientQuery>(
			document,
			views::management::transfer_recipients,
		)?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::management::OperationPage>(
			document,
			views::management::operation_history,
			200,
			"application/json",
		)?;
	contracts.path(document, views::management::operation_history, &["Uuid"])?;
	contracts.query::<_, crate::apps::execution::capabilities::serializers::endpoints::Page>(
		document,
		views::management::operation_history,
	)?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::endpoints::ReferencePage>(
			document,
			views::management::reference_list,
			200,
			"application/json",
		)?;
	contracts.query::<_, crate::apps::execution::capabilities::serializers::endpoints::Page>(
		document,
		views::management::reference_list,
	)?;
	contracts
		.response::<_, crate::apps::execution::capabilities::serializers::management::DeletedThread>(
			document,
			views::management::thread_delete,
			200,
			"application/json",
		)?;
	contracts
		.request::<_, crate::apps::execution::capabilities::serializers::thread_lifecycle::DeleteThread>(
			document,
			views::management::thread_delete,
		)?;
	contracts.path(
		document,
		views::management::thread_delete,
		&["Uuid", "Uuid"],
	)?;
	Ok(())
}
