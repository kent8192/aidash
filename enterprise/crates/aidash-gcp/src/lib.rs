// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! Google Cloud adapters that only the Cloud and Dedicated Offerings use.
#[cfg(feature = "cloud")]
pub mod capability;
pub mod gcip;
#[cfg(feature = "cloud")]
pub mod secret_manager;
