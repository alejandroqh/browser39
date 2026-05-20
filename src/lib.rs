pub mod cli;
pub mod core;
pub mod mcp;
pub mod service;

pub use core::config::{Config, PersistenceMode};
pub use core::page::{FetchOptions, HttpMethod};
pub use core::session_store::{InMemoryStore, SessionStore, create_session_store};
pub use service::service::BrowserService;
