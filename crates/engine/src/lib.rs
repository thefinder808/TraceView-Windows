//! Log engine with no UI dependency: positional file reads, a compact row
//! index, on-demand row parsing, parallel search, summaries, and the counts
//! behind the severity chips and event histogram. The GUI, CLI and a future
//! MCP server are all thin layers over this crate.

#![forbid(unsafe_code)]

pub mod cache;
pub mod custom;
pub mod encoding;
pub mod file;
pub mod index;
pub mod level;
pub mod merge;
pub mod overview;
pub mod parse;
pub mod query;
mod reader;
pub mod summary;
pub mod time;

pub use custom::CustomFormat;
pub use encoding::Encoding;
pub use file::{CacheUse, Change, LogFile, OpenStats, RefreshPlan};
pub use level::Level;
pub use overview::{Histogram, SpikeMemory, TimeRange};
pub use parse::{Fields, Format};
pub use query::{Control, Query, QueryError, QueryResult, RowRecord};
pub use summary::{LevelCounts, Pattern, Spike, Summary};
