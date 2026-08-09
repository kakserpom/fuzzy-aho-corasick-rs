//! Enable the book-example doctests (the `book_doctests` module in `lib.rs`) only when the book
//! sources are present — i.e. in this repository, not in the packaged crate, which `exclude`s
//! `/book`. Without this, the published crate's `include_str!("../book/...")` would fail to compile
//! under `cargo test --doc`.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(book_present)");
    if std::path::Path::new("book/src").exists() {
        println!("cargo::rustc-cfg=book_present");
    }
    println!("cargo::rerun-if-changed=book/src");
}
