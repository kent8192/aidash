//! OpenAPI payload contracts for native endpoints.
use super::super::views;
use crate::apps::registry::workbench::serializers::audit::AuditPage;
use crate::apps::registry::workbench::serializers::audit::AuditQuery;
use crate::apps::registry::workbench::serializers::contracts::AdoptInput;
use crate::apps::registry::workbench::serializers::contracts::ArchiveInput;
use crate::apps::registry::workbench::serializers::contracts::CreateDraft;
use crate::apps::registry::workbench::serializers::contracts::Draft;
use crate::apps::registry::workbench::serializers::contracts::DraftShare;
use crate::apps::registry::workbench::serializers::contracts::RegisteredVersion;
use crate::apps::registry::workbench::serializers::contracts::Registration as WorkbenchContractsRegistration;
use crate::apps::registry::workbench::serializers::contracts::RevisionInput;
use crate::apps::registry::workbench::serializers::contracts::SaveDraft;
use crate::apps::registry::workbench::serializers::contracts::ShareInput;
use crate::apps::registry::workbench::serializers::contracts::TransferInput;
use crate::apps::registry::workbench::serializers::contracts::Validation;
use crate::apps::registry::workbench::serializers::incident::CreateIncident;
use crate::apps::registry::workbench::serializers::incident::Incident;
use crate::apps::registry::workbench::serializers::incident::IncidentEvent;
use crate::apps::registry::workbench::serializers::incident::UpdateIncident;
use crate::apps::registry::workbench::serializers::profile::ProfileInput;
use crate::apps::registry::workbench::serializers::profile::ProfileSummary;
use crate::apps::registry::workbench::serializers::profile::TestProfile;
use crate::apps::registry::workbench::serializers::test::TestInput;
use crate::apps::registry::workbench::serializers::test::TestLimits;
use crate::apps::registry::workbench::serializers::test::TestSession;
use crate::apps::registry::workbench::serializers::trust::Inspection as WorkbenchTrustInspection;
use crate::apps::registry::workbench::serializers::trust::PermissionContext;
use crate::apps::registry::workbench::serializers::trust::PermissionInput;
use crate::{Result, config::openapi::Contracts};
use reinhardt::rest::openapi::OpenApiSchema;

pub(crate) fn register(contracts: &mut Contracts, document: &mut OpenApiSchema) -> Result<()> {
	contracts.response::<_, Draft>(document, views::drafts::create, 200, "application/json")?;
	contracts.request::<_, CreateDraft>(document, views::drafts::create)?;
	contracts.response::<_, Vec<Draft>>(document, views::drafts::list, 200, "application/json")?;
	contracts.response::<_, Draft>(document, views::drafts::get, 200, "application/json")?;
	contracts.path(document, views::drafts::get, &["Uuid"])?;
	contracts.response::<_, Draft>(document, views::drafts::save, 200, "application/json")?;
	contracts.request::<_, SaveDraft>(document, views::drafts::save)?;
	contracts.path(document, views::drafts::save, &["Uuid"])?;
	contracts.response::<_, Draft>(document, views::drafts::duplicate, 200, "application/json")?;
	contracts.request::<_, RevisionInput>(document, views::drafts::duplicate)?;
	contracts.path(document, views::drafts::duplicate, &["Uuid"])?;
	contracts.response::<_, Draft>(document, views::drafts::adopt, 200, "application/json")?;
	contracts.request::<_, AdoptInput>(document, views::drafts::adopt)?;
	contracts.path(document, views::drafts::adopt, &["String", "String"])?;
	contracts.response::<_, Draft>(document, views::drafts::share, 200, "application/json")?;
	contracts.request::<_, ShareInput>(document, views::drafts::share)?;
	contracts.path(document, views::drafts::share, &["Uuid"])?;
	contracts.response::<_, Vec<DraftShare>>(
		document,
		views::drafts::shares,
		200,
		"application/json",
	)?;
	contracts.path(document, views::drafts::shares, &["Uuid"])?;
	contracts.response::<_, Draft>(document, views::drafts::transfer, 200, "application/json")?;
	contracts.request::<_, TransferInput>(document, views::drafts::transfer)?;
	contracts.path(document, views::drafts::transfer, &["Uuid"])?;
	contracts.response::<_, Draft>(document, views::drafts::archive, 200, "application/json")?;
	contracts.request::<_, ArchiveInput>(document, views::drafts::archive)?;
	contracts.path(document, views::drafts::archive, &["Uuid"])?;
	contracts.response::<_, Validation>(
		document,
		views::drafts::validate,
		200,
		"application/json",
	)?;
	contracts.request::<_, RevisionInput>(document, views::drafts::validate)?;
	contracts.path(document, views::drafts::validate, &["Uuid"])?;
	contracts.response::<_, Vec<RegisteredVersion>>(
		document,
		views::drafts::versions,
		200,
		"application/json",
	)?;
	contracts.path(document, views::drafts::versions, &["Uuid"])?;
	contracts.response::<_, WorkbenchContractsRegistration>(
		document,
		views::drafts::register,
		200,
		"application/json",
	)?;
	contracts.request::<_, RevisionInput>(document, views::drafts::register)?;
	contracts.path(document, views::drafts::register, &["Uuid"])?;
	contracts.response::<_, AuditPage>(document, views::audit::audit, 200, "application/json")?;
	contracts.query::<_, AuditQuery>(document, views::audit::audit)?;
	contracts.path(document, views::audit::audit, &["String", "String"])?;
	contracts.response::<_, Incident>(
		document,
		views::incident::create,
		200,
		"application/json",
	)?;
	contracts.request::<_, CreateIncident>(document, views::incident::create)?;
	contracts.path(document, views::incident::create, &["String", "String"])?;
	contracts.response::<_, Vec<Incident>>(
		document,
		views::incident::list,
		200,
		"application/json",
	)?;
	contracts.path(document, views::incident::list, &["String", "String"])?;
	contracts.response::<_, Incident>(document, views::incident::get, 200, "application/json")?;
	contracts.path(document, views::incident::get, &["Uuid"])?;
	contracts.response::<_, Incident>(
		document,
		views::incident::update,
		200,
		"application/json",
	)?;
	contracts.request::<_, UpdateIncident>(document, views::incident::update)?;
	contracts.path(document, views::incident::update, &["Uuid"])?;
	contracts.response::<_, Vec<IncidentEvent>>(
		document,
		views::incident::events,
		200,
		"application/json",
	)?;
	contracts.path(document, views::incident::events, &["Uuid"])?;
	contracts.response::<_, Vec<ProfileSummary>>(
		document,
		views::profile::list,
		200,
		"application/json",
	)?;
	contracts.response::<_, TestProfile>(document, views::profile::put, 200, "application/json")?;
	contracts.request::<_, ProfileInput>(document, views::profile::put)?;
	contracts.path(document, views::profile::put, &["String", "String"])?;
	contracts.response::<_, TestLimits>(
		document,
		views::test::get_limits,
		200,
		"application/json",
	)?;
	contracts.path(document, views::test::get_limits, &["Uuid"])?;
	contracts.response::<_, TestLimits>(
		document,
		views::test::set_limits,
		200,
		"application/json",
	)?;
	contracts.request::<_, TestLimits>(document, views::test::set_limits)?;
	contracts.path(document, views::test::set_limits, &["String"])?;
	contracts.response::<_, Vec<TestSession>>(
		document,
		views::test::sessions,
		200,
		"application/json",
	)?;
	contracts.path(document, views::test::sessions, &["Uuid"])?;
	contracts.response::<_, TestSession>(document, views::test::stop, 200, "application/json")?;
	contracts.path(document, views::test::stop, &["Uuid"])?;
	contracts.response::<_, TestSession>(document, views::test::start, 200, "application/json")?;
	contracts.request::<_, TestInput>(document, views::test::start)?;
	contracts.path(document, views::test::start, &["Uuid"])?;
	contracts.response::<_, WorkbenchTrustInspection>(
		document,
		views::trust::inspect,
		200,
		"application/json",
	)?;
	contracts.path(document, views::trust::inspect, &["String", "String"])?;
	contracts.response::<_, PermissionContext>(
		document,
		views::trust::permission_context,
		200,
		"application/json",
	)?;
	contracts.request::<_, PermissionInput>(document, views::trust::permission_context)?;
	contracts.path(
		document,
		views::trust::permission_context,
		&["String", "String"],
	)?;
	contracts.empty(document, views::trust::report, 200)?;
	contracts.path(document, views::trust::report, &["String", "String"])?;
	Ok(())
}
