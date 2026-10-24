//! activation application module
//!
//! A RESTful API application

pub mod admin;
pub mod models;
pub(crate) mod repositories;
pub mod serializers;
pub mod services;
pub mod urls;
pub mod views;

pub use services::*;
