//! marketplace application module
//!
//! A RESTful API application

pub mod admin;
pub mod models;
pub mod serializers;
pub mod services;
pub mod urls;
pub mod views;

use reinhardt::app_config;
#[app_config(name = "marketplace", label = "marketplace")]
pub struct MarketplaceConfig;

#[cfg(test)]
mod tests;

pub(crate) mod repositories;
