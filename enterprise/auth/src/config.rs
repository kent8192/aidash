// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! Project settings and explicit full-stack app registration.
pub mod apps;
#[cfg(server)]
pub mod settings;
pub mod urls;
#[cfg(server)]
pub use settings::{Client, Config, Grpc};
