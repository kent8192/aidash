//! Native management-process lifecycle coverage with disposable Reinhardt fixtures.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
#[path = "../../identity/tests/deployment.rs"]
mod dashboard;
#[path = "database_roles.rs"]
mod database_roles;
#[path = "support/deployment.rs"]
mod deployment;
#[path = "startup.rs"]
mod harness;
#[path = "support/manage.rs"]
mod manage;
#[path = "support/native_database.rs"]
mod native_database;
#[path = "tenancy.rs"]
mod tenancy;
