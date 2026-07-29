//! Internal prelude — the crate's common working types (the whole `structs` set plus the
//! grapheme-storage abstraction), gathered so the engine's modules can `use crate::prelude::*`
//! instead of maintaining long explicit import lists. Glob imports from a `prelude` module are
//! exempt from Clippy's `wildcard_imports` lint.
pub(crate) use crate::grapheme::{AsciiGraphemes, GraphemeStorage};
pub(crate) use crate::structs::*;
