//! Orchestration application.

pub mod admin;
pub mod models;
pub mod serializers;
pub mod services;
pub mod urls;
pub mod views;

pub use services::*;

use reinhardt::app_config;
#[app_config(name = "operations", label = "operations")]
pub struct OperationsConfig;

#[cfg(test)]
mod tests;
