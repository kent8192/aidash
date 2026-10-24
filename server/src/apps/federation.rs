//! federation application module
//!
//! A RESTful API application

use reinhardt::app_config;

pub mod admin;
pub mod models;
pub mod serializers;
pub mod services;
pub mod urls;
pub mod views;

#[app_config(name = "federation", label = "federation")]
pub struct FederationConfig;

pub mod peer;

pub mod remote;

pub mod transactions;

#[cfg(test)]
mod tests;
