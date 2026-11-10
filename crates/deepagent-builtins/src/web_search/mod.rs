//! One callable search tool; engine protocols do not call a language model.
#![allow(missing_docs)]
pub mod types;

#[cfg(feature = "http")]
pub mod dispatcher;
#[cfg(feature = "http")]
pub mod engines;
#[cfg(feature = "http")]
pub mod normalize;
#[cfg(feature = "http")]
pub mod tool;

#[cfg(feature = "http")]
pub use dispatcher::{ReqwestSearchEngineClient, SearchEngineClient};
#[cfg(feature = "http")]
pub use tool::WebSearchTool;

#[cfg(feature = "http")]
pub use types::SearchConfigSource;
