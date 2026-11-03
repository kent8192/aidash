// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! Native protocol, persistence and transport adapters.
pub(crate) mod assets;
pub(crate) mod auth;
pub mod migrations;
pub mod rpc;
pub(crate) mod store;
#[cfg(test)]
mod tests;
pub(crate) mod views;
