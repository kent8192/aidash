//! Native management-process lifecycle coverage with disposable Reinhardt fixtures.
#[path = "../../identity/tests/deployment.rs"]
mod dashboard;
#[path = "support/deployment.rs"]
mod deployment;
#[path = "startup.rs"]
mod harness;
#[path = "support/manage.rs"]
mod manage;
#[path = "support/native_database.rs"]
mod native_database;
