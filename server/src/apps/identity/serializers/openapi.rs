//! OpenAPI payload contracts for native endpoints.
use super::super::views;
use crate::apps::identity::serializers::catalog::Binding;
use crate::apps::identity::serializers::contracts::Snapshot;
use crate::apps::identity::serializers::identity::Credential;
use crate::apps::identity::serializers::identity::IssuedCredential;
use crate::apps::identity::serializers::peer::HistoryPage;
use crate::apps::identity::serializers::peer::MappingPage;
use crate::apps::identity::serializers::peer::MappingRevision as AuthorizationPeerMappingRevision;
use crate::apps::identity::serializers::peer::PeerMapping;
use crate::apps::identity::serializers::peer::PeerMappingInput;
use crate::apps::identity::serializers::policies::AuthorizationPage;
use crate::apps::identity::serializers::policies::AuthorizationUpdate;
use crate::apps::identity::serializers::policies::CatalogInput;
use crate::apps::identity::serializers::policies::CredentialInput;
use crate::apps::identity::serializers::remote::PrepareInput;
use crate::apps::identity::serializers::remote::Prepared;
use crate::apps::identity::serializers::services_policy::Decision;
use crate::apps::identity::serializers::services_policy::Evaluation;
use crate::apps::identity::serializers::session::SessionResponse;
use crate::apps::workspaces::serializers::tasks::PageQuery;
use crate::{Result, config::openapi::Contracts};
use reinhardt::rest::openapi::OpenApiSchema;
use serde_json::Value;

pub(crate) fn register(contracts: &mut Contracts, document: &mut OpenApiSchema) -> Result<()> {
	use super::desktop;
	contracts.response::<_, desktop::Started>(
		document,
		views::desktop::start,
		200,
		"application/json",
	)?;
	contracts.request::<_, desktop::Start>(document, views::desktop::start)?;
	contracts.response::<_, String>(document, views::desktop::authorize, 200, "text/html")?;
	contracts.query::<_, desktop::AuthorizationRequest>(document, views::desktop::authorize)?;
	contracts.empty(document, views::desktop::consent, 307)?;
	contracts.response_header::<_, String>(
		document,
		views::desktop::consent,
		307,
		"Location",
		"Bound loopback callback",
	)?;
	contracts.form::<_, desktop::Consent>(document, views::desktop::consent)?;
	contracts.response::<_, desktop::Tokens>(
		document,
		views::desktop::exchange,
		200,
		"application/json",
	)?;
	contracts.request::<_, desktop::Exchange>(document, views::desktop::exchange)?;
	contracts.response::<_, desktop::Tokens>(
		document,
		views::desktop::refresh,
		200,
		"application/json",
	)?;
	contracts.request::<_, desktop::Renewal>(document, views::desktop::refresh)?;
	contracts.empty(document, views::desktop::revoke, 204)?;
	contracts.request::<_, desktop::Revocation>(document, views::desktop::revoke)?;
	contracts.response::<_, SessionResponse>(
		document,
		views::management::session,
		200,
		"application/json",
	)?;
	contracts.response::<_, Vec<Binding>>(
		document,
		views::policies::catalog,
		200,
		"application/json",
	)?;
	contracts.path(document, views::policies::catalog, &["String"])?;
	contracts.response::<_, Binding>(
		document,
		views::policies::set_catalog,
		200,
		"application/json",
	)?;
	contracts.request::<_, CatalogInput>(document, views::policies::set_catalog)?;
	contracts.path(document, views::policies::set_catalog, &["String"])?;
	contracts.response::<_, IssuedCredential>(
		document,
		views::policies::issue_credential,
		200,
		"application/json",
	)?;
	contracts.request::<_, CredentialInput>(document, views::policies::issue_credential)?;
	contracts.path(document, views::policies::issue_credential, &["String"])?;
	contracts.response::<_, Vec<Credential>>(
		document,
		views::policies::credentials,
		200,
		"application/json",
	)?;
	contracts.path(document, views::policies::credentials, &["String"])?;
	contracts.query::<_, PageQuery>(document, views::policies::credentials)?;
	contracts.response::<_, super::policies::CredentialRevocation>(
		document,
		views::policies::revoke_credential,
		200,
		"application/json",
	)?;
	contracts.path(
		document,
		views::policies::revoke_credential,
		&["String", "Uuid"],
	)?;
	contracts.response::<_, Snapshot>(
		document,
		views::policies::snapshot,
		200,
		"application/json",
	)?;
	contracts.path(document, views::policies::snapshot, &["String"])?;
	contracts.response::<_, super::policies::PolicyReplacement>(
		document,
		views::policies::replace,
		200,
		"application/json",
	)?;
	contracts.request::<_, AuthorizationUpdate>(document, views::policies::replace)?;
	contracts.path(document, views::policies::replace, &["String"])?;
	contracts.response::<_, Decision>(
		document,
		views::policies::evaluate,
		200,
		"application/json",
	)?;
	contracts.request::<_, Evaluation>(document, views::policies::evaluate)?;
	contracts.path(document, views::policies::evaluate, &["String"])?;
	contracts.response::<_, Decision>(
		document,
		views::policies::simulate,
		200,
		"application/json",
	)?;
	contracts.request::<_, Evaluation>(document, views::policies::simulate)?;
	contracts.path(document, views::policies::simulate, &["String"])?;
	contracts.response::<_, Vec<Value>>(
		document,
		views::policies::revisions,
		200,
		"application/json",
	)?;
	contracts.query::<_, AuthorizationPage>(document, views::policies::revisions)?;
	contracts.path(document, views::policies::revisions, &["String"])?;
	contracts.response::<_, Vec<Value>>(
		document,
		views::policies::decisions,
		200,
		"application/json",
	)?;
	contracts.query::<_, AuthorizationPage>(document, views::policies::decisions)?;
	contracts.path(document, views::policies::decisions, &["String"])?;
	contracts.response::<_, Vec<AuthorizationPeerMappingRevision>>(
		document,
		views::peer_mappings::history,
		200,
		"application/json",
	)?;
	contracts.query::<_, HistoryPage>(document, views::peer_mappings::history)?;
	contracts.path(document, views::peer_mappings::history, &["String"])?;
	contracts.response::<_, Vec<PeerMapping>>(
		document,
		views::peer_mappings::list,
		200,
		"application/json",
	)?;
	contracts.query::<_, MappingPage>(document, views::peer_mappings::list)?;
	contracts.path(document, views::peer_mappings::list, &["String"])?;
	contracts.response::<_, PeerMapping>(
		document,
		views::peer_mappings::set,
		200,
		"application/json",
	)?;
	contracts.request::<_, PeerMappingInput>(document, views::peer_mappings::set)?;
	contracts.path(document, views::peer_mappings::set, &["String"])?;
	contracts.response::<_, Prepared>(
		document,
		views::remote_grants::prepare,
		200,
		"application/json",
	)?;
	contracts.request::<_, PrepareInput>(document, views::remote_grants::prepare)?;
	contracts.path(document, views::remote_grants::prepare, &["Uuid"])?;
	contracts.response::<_, Prepared>(
		document,
		views::remote_grants::revoke,
		200,
		"application/json",
	)?;
	contracts.path(document, views::remote_grants::revoke, &["Uuid", "Uuid"])?;
	contracts.response::<_, Vec<crate::apps::identity::serializers::peer_graph::GraphPeer>>(
		document,
		views::graph::peers,
		200,
		"application/json",
	)?;
	contracts.response::<_, crate::apps::identity::serializers::peer_graph::GraphPage>(
		document,
		views::graph::expand,
		200,
		"application/json",
	)?;
	contracts.request::<_, crate::apps::identity::serializers::peer_graph::GraphExpandInput>(
		document,
		views::graph::expand,
	)?;
	contracts
		.response::<_, Vec<crate::apps::identity::serializers::peer_graph::GraphOperatorGrant>>(
			document,
			views::graph::list_grants,
			200,
			"application/json",
		)?;
	contracts.path(document, views::graph::list_grants, &["String"])?;
	contracts.query::<_, crate::apps::identity::serializers::peer_graph::GrantPage>(
		document,
		views::graph::list_grants,
	)?;
	contracts.response::<_, crate::apps::identity::serializers::peer_graph::GraphOperatorGrant>(
		document,
		views::graph::set_grant,
		200,
		"application/json",
	)?;
	contracts
		.request::<_, crate::apps::identity::serializers::peer_graph::GraphOperatorGrantInput>(
			document,
			views::graph::set_grant,
		)?;
	contracts.path(document, views::graph::set_grant, &["String"])?;
	contracts.response::<_, crate::apps::identity::serializers::remote_execution::RemoteExecutionMessageReceipt>(document,views::remote_execution::message,200,"application/json")?;
	contracts
		.request::<_, crate::apps::identity::serializers::remote_execution::RemoteExecutionMessageInput>(
			document,
			views::remote_execution::message,
		)?;
	contracts.path(
		document,
		views::remote_execution::message,
		&["Uuid", "Uuid"],
	)?;
	contracts
		.response::<_, crate::apps::identity::serializers::remote_execution::RemoteExecutionActivation>(
			document,
			views::remote_execution::control,
			200,
			"application/json",
		)?;
	contracts
		.request::<_, crate::apps::identity::serializers::peer_admission::RemoteExecutionControlInput>(
			document,
			views::remote_execution::control,
		)?;
	contracts.path(
		document,
		views::remote_execution::control,
		&["Uuid", "Uuid"],
	)?;
	contracts
		.response::<_, Vec<crate::apps::identity::serializers::remote_execution::RemoteExecutionStatus>>(
			document,
			views::remote_execution::list,
			200,
			"application/json",
		)?;
	contracts.path(document, views::remote_execution::list, &["Uuid"])?;
	contracts.response::<_, aidash_domain::HumanRequest>(
		document,
		views::remote_execution::answer_human,
		200,
		"application/json",
	)?;
	contracts
		.request::<_, crate::apps::identity::serializers::remote_execution::RemoteHumanAnswer>(
			document,
			views::remote_execution::answer_human,
		)?;
	contracts.path(
		document,
		views::remote_execution::answer_human,
		&["Uuid", "Uuid"],
	)?;
	contracts
		.response::<_, crate::apps::identity::serializers::remote_execution::RemoteExecutionActivation>(
			document,
			views::remote_execution::activate,
			200,
			"application/json",
		)?;
	contracts.path(
		document,
		views::remote_execution::activate,
		&["Uuid", "Uuid"],
	)?;
	contracts.response::<_, crate::apps::identity::serializers::peer_graph::GraphPage>(
		document,
		views::graph::project,
		200,
		"application/json",
	)?;
	contracts.request::<_, crate::apps::identity::serializers::peer_graph::GraphRequest>(
		document,
		views::graph::project,
	)?;
	contracts
		.response::<_, crate::apps::identity::serializers::remote_execution::RemoteExecutionActivation>(
			document,
			views::peer_execution::activate,
			200,
			"application/json",
		)?;
	contracts.request::<_, crate::apps::identity::serializers::peer_admission::Input>(
		document,
		views::peer_execution::activate,
	)?;
	contracts.path(document, views::peer_execution::activate, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::identity::serializers::remote_execution::RemoteExecutionActivation>(
			document,
			views::peer_execution::control,
			200,
			"application/json",
		)?;
	contracts
		.request::<_, crate::apps::identity::serializers::peer_admission::RemoteExecutionControlInput>(
			document,
			views::peer_execution::control,
		)?;
	contracts.path(document, views::peer_execution::control, &["Uuid"])?;
	contracts.response::<_, crate::apps::identity::serializers::remote_execution::RemoteExecutionMessageReceipt>(document,views::peer_execution::message,200,"application/json")?;
	contracts.request::<_, crate::apps::identity::serializers::peer_admission::MessageInput>(
		document,
		views::peer_execution::message,
	)?;
	contracts.path(document, views::peer_execution::message, &["Uuid"])?;
	contracts.response::<_, Option<
		crate::apps::identity::serializers::remote_execution::RemoteExecutionActivation,
	>>(
		document,
		views::peer_execution::status,
		200,
		"application/json",
	)?;
	contracts.request::<_, crate::apps::identity::serializers::peer_admission::Input>(
		document,
		views::peer_execution::status,
	)?;
	contracts.response::<_, serde_json::Value>(
		document,
		views::remote_commands::handle,
		200,
		"application/json",
	)?;
	contracts.request::<_, crate::apps::identity::serializers::remote_execution_commands::Input>(
		document,
		views::remote_commands::handle,
	)?;
	contracts.response::<_, super::policies::CredentialRevocation>(
		document,
		views::policies::revoke_credential,
		202,
		"application/json",
	)?;
	contracts.response::<_, super::policies::PolicyReplacement>(
		document,
		views::policies::replace,
		202,
		"application/json",
	)?;
	contracts.response::<_, Vec<uuid::Uuid>>(
		document,
		views::policies::transaction_revocations,
		200,
		"application/json",
	)?;
	contracts.path(
		document,
		views::policies::transaction_revocations,
		&["String"],
	)?;
	contracts.response::<_, uuid::Uuid>(
		document,
		views::remote_execution::activation_binding,
		200,
		"application/json",
	)?;
	contracts.request::<_, super::remote::VerifyInput>(
		document,
		views::remote_execution::activation_binding,
	)?;
	contracts.response::<_, crate::authorization::execution::management::RunManagement>(
		document,
		views::state_management::get,
		200,
		"application/json",
	)?;
	contracts.path(document, views::state_management::get, &["Uuid"])?;
	contracts.response::<_, crate::authorization::execution::management::RunManagement>(
		document,
		views::state_management::control,
		200,
		"application/json",
	)?;
	contracts.request::<_, crate::authorization::execution::management::RunManagementInput>(
		document,
		views::state_management::control,
	)?;
	contracts.path(document, views::state_management::control, &["Uuid"])?;
	contracts.response::<_, Option<crate::semantic::remote::status::Provenance>>(
		document,
		views::provenance::provenance,
		200,
		"application/json",
	)?;
	contracts.path(document, views::provenance::provenance, &["Uuid", "Uuid"])?;
	contracts.response::<_, aidash_domain::Task>(
		document,
		views::provenance::follow_up,
		200,
		"application/json",
	)?;
	contracts.request::<_, crate::authorization::remote::execution::FollowUpInput>(
		document,
		views::provenance::follow_up,
	)?;
	contracts.path(document, views::provenance::follow_up, &["Uuid", "Uuid"])?;
	Ok(())
}
