//! Lets people talk to minerve from chat apps (Slack today). Embedded by minerve's TUI through
//! [`service::Gateway`]; `minerve-gateway` runs it on its own.

pub mod engine;
pub mod service;
pub mod transport;
