//! Document model and Markdown parsing for the diff viewer's preview pane.
//!
//! The crate is pure: no GPUI, no gix, no I/O. Input is source text some other
//! crate already read, output is an owned block tree a renderer walks.

mod model;
mod parse;

pub use model::{CellAlignment, DocBlock, DocBlockKind, DocSpan, DocumentKind, image_sources};
pub use parse::parse_markdown;
