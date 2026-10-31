// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! Service modules for the authentication application.
//!
//! Client-only services live under `services/client/`; server-only services
//! live under `services/server/`.

#[cfg(client)]
pub mod client;

#[cfg(server)]
pub mod server;
