// SPDX-License-Identifier: GPL-3.0-only

//! Pencil — a rich text editor for the COSMIC desktop.
//!
//! A library beside the binary so the parts with rules of their own — what a
//! format keeps, what a save guarantees — can be tested without a window.

pub mod app;
pub mod chrome;
pub mod config;
pub mod document;
pub mod i18n;

pub use app::{APP_ID, App};
