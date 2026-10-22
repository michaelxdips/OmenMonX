//! UI module tree.
//!
//! `App` stays the single owner of state; these modules only render it. That is
//! the point of the split: before it, one 1759-line file held the shell, five
//! pages, every card and `main()`, so a restyle meant editing the same file as a
//! behaviour fix and neither could be reviewed on its own.

pub mod components;
pub mod pages;
pub mod shell;
