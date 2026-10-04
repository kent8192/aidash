//! Harness application.

pub mod admin;
pub mod models;
pub mod repositories;
pub mod serializers;
pub mod services;
pub mod urls;
pub mod views;

pub use services::*;

#[cfg(test)]
#[path = "execution/tests/openapi.rs"]
mod openapi_tests;

use reinhardt::app_config;
#[app_config(name = "execution", label = "execution")]
pub struct ExecutionConfig;

pub mod capabilities;

pub mod activation;

pub mod generation;

#[cfg(test)]
mod tests;
