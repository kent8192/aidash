//! Native draft adapters keep ORM rows and portable state separate.
pub mod authority;

pub mod drafts;

pub(crate) mod audit;

pub(crate) mod permissions;

pub(crate) mod inspection;

pub(crate) mod incidents;

pub(crate) mod report;

pub(crate) mod profile;

pub(crate) mod sandbox;
