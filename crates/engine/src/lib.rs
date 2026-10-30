//! Log engine with no UI dependency: memory-mapped files, a compact row index,
//! on-demand row parsing, and parallel regex filtering.

pub mod cache;
pub mod file;
pub mod filter;
pub mod index;
pub mod parse;

pub use file::{LogFile, OpenStats};
pub use index::{Format, Level, RowIndex};
