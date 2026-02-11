// Allow pre-existing clippy lints across the codebase
#![allow(
    clippy::collapsible_if,
    clippy::unnecessary_map_or,
    clippy::redundant_closure,
    clippy::manual_strip,
    clippy::needless_range_loop,
    clippy::identity_op,
    clippy::len_without_is_empty,
    clippy::doc_lazy_continuation,
    clippy::empty_line_after_doc_comments,
    clippy::implicit_saturating_sub,
    clippy::manual_div_ceil,
    clippy::manual_range_contains,
    clippy::needless_lifetimes,
    clippy::needless_return,
    clippy::too_many_arguments
)]

pub mod base64;
pub mod common;
pub mod cut;
pub mod hash;
pub mod sort;
pub mod tac;
pub mod tr;
pub mod uniq;
pub mod wc;
