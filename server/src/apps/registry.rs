//! Registry application.

pub mod admin;
pub mod models;
pub mod serializers;
pub mod services;
pub mod urls;
pub mod views;

pub use services::*;

use reinhardt::app_config;
#[app_config(name = "registry", label = "registry")]
pub struct RegistryConfig;

pub mod workbench;

pub mod repositories;

#[cfg(test)]
mod tests;
