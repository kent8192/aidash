//! Collaboration application.

pub mod admin;
pub mod models;
pub mod serializers;
pub mod services;
pub mod urls;
pub mod views;

pub use services::*;

use reinhardt::app_config;
#[app_config(name = "workspaces", label = "workspaces")]
pub struct WorkspacesConfig;

pub mod sse;

#[cfg(test)]
mod tests;

pub mod repositories;
