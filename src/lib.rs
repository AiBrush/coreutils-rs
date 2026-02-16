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
    clippy::needless_borrows_for_generic_args,
    clippy::needless_lifetimes,
    clippy::needless_return,
    clippy::too_many_arguments,
    clippy::unnecessary_cast,
    clippy::write_literal,
    clippy::io_other_error
)]

/// Use mimalloc as the global allocator for all binaries.
/// 2-3x faster than glibc malloc for small allocations,
/// better thread-local caching, and reduced fragmentation.
/// Critical for tools like sort/uniq that do many small allocs.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

pub mod base64;
pub mod cat;
#[cfg(unix)]
pub mod chgrp;
#[cfg(unix)]
pub mod chmod;
#[cfg(unix)]
pub mod chown;
pub mod comm;
pub mod common;
#[cfg(unix)]
pub mod cp;
pub mod csplit;
pub mod cut;
pub mod date;
pub mod dd;
#[cfg(unix)]
pub mod df;
pub mod du;
pub mod echo;
pub mod expand;
pub mod expr;
pub mod factor;
pub mod fmt;
pub mod fold;
pub mod hash;
pub mod head;
#[cfg(unix)]
pub mod install;
pub mod join;
#[cfg(unix)]
pub mod ls;
#[cfg(unix)]
pub mod mv;
pub mod nl;
pub mod numfmt;
pub mod od;
pub mod paste;
#[cfg(unix)]
pub mod pinky;
pub mod pr;
pub mod printf;
pub mod ptx;
pub mod rev;
#[cfg(unix)]
pub mod rm;
pub mod shred;
pub mod sort;
pub mod split;
#[cfg(unix)]
pub mod stat;
#[cfg(unix)]
pub mod stdbuf;
pub mod stty;
pub mod tac;
pub mod tail;
pub mod test_cmd;
pub mod tr;
pub mod uniq;
#[cfg(unix)]
pub mod users;
pub mod wc;
#[cfg(unix)]
pub mod who;
