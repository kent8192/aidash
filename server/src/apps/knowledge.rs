//! Semantic application.

pub mod admin;
pub mod models;
pub mod serializers;
pub mod services;
pub mod urls;
pub mod views;

pub use services::*;

use reinhardt::app_config;
#[app_config(name = "knowledge", label = "knowledge")]
pub struct KnowledgeConfig;

pub(crate) use services::remote;

#[cfg(test)]
mod tests;

pub(crate) mod repositories;
