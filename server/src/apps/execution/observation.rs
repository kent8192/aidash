//! Tenant opt-in observation is independent of approval settlement and Run leases.
mod journal;
mod repository;
mod views;
mod worker;
pub(crate) use repository::{enqueue_capability, enqueue_generation, label};
pub(crate) use views::{register, routes};
pub(crate) use worker::{provider_scope, run};
#[cfg(test)]
mod tests;
