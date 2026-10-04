//! Django-style endpoint suites live beside the application they exercise.
#[path = "support/endpoint.rs"]
mod endpoint;
#[path = "support/execution.rs"]
mod execution_fixtures;
#[path = "support/isolated.rs"]
mod isolated;
#[path = "support/native_database.rs"]
mod native_database;
// Endpoint scenarios use successful inference; adapter suites cover other replies.
#[allow(dead_code)]
#[path = "provider_fixtures.rs"]
mod provider_fixtures;

#[path = "../../identity/tests/endpoints.rs"]
mod authorization;
#[path = "../../workspaces/tests/endpoints.rs"]
mod collaboration;
#[path = "../../workspaces/tests/catalog_endpoints.rs"]
mod collaboration_catalog;
#[path = "../../workspaces/tests/graph_endpoints.rs"]
mod collaboration_graph;
#[path = "frontend.rs"]
mod frontend;
#[path = "../generation/tests/endpoints.rs"]
mod generation;
#[path = "endpoints.rs"]
mod harness;
#[path = "interaction_endpoints.rs"]
mod harness_interaction;
#[path = "lifecycle.rs"]
mod lifecycle;
#[path = "../../operations/tests/endpoints.rs"]
mod orchestration;
#[path = "../../federation/peer/tests/endpoints.rs"]
mod peer;
#[path = "../../registry/tests/endpoints.rs"]
mod registry;
#[path = "../../registry/tests/validation.rs"]
mod registry_validation;
#[path = "../../federation/remote/tests/endpoints.rs"]
mod remote;
#[path = "../../knowledge/tests/endpoints.rs"]
mod semantic;
#[path = "../../federation/transactions/tests/endpoints.rs"]
mod transactions;
#[path = "../../federation/transactions/tests/execution_endpoints.rs"]
mod transactions_execution;
#[path = "../../registry/workbench/tests/endpoints.rs"]
mod workbench;

#[path = "../../identity/tests/openapi.rs"]
mod authorization_openapi;
#[path = "../../identity/tests/policies.rs"]
mod authorization_policies;

#[path = "../../workspaces/tests/channel_attachments.rs"]
mod channel_attachments;
#[path = "../../workspaces/tests/channel_fixtures.rs"]
mod channel_fixtures;
#[path = "../../workspaces/tests/channel_threads.rs"]
mod channel_threads;

#[path = "../../federation/peer/tests/registration.rs"]
mod peer_registration;

#[path = "../../federation/peer/tests/journals.rs"]
mod peer_journals;
