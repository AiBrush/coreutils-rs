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

/// Use mimalloc as the global allocator for all binaries.
/// 2-3x faster than glibc malloc for small allocations,
/// better thread-local caching, and reduced fragmentation.
/// Critical for tools like sort/uniq that do many small allocs.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

pub mod base64;
pub mod common;
pub mod cut;
pub mod hash;
pub mod sort;
pub mod tac;
pub mod tr;
pub mod uniq;
pub mod wc;
