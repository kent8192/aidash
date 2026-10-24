//! Authorization application.

pub mod admin;
pub mod models;
pub mod repositories;
pub mod serializers;
pub mod services;
pub mod urls;
pub mod views;

pub use services::*;

use reinhardt::app_config;
#[app_config(name = "identity", label = "identity")]
pub struct IdentityConfig;

#[cfg(test)]
mod tests;
