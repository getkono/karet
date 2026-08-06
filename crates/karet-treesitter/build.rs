//! Compile generated parsers that do not have compatible crates.io packages.

use std::path::Path;

fn compile(name: &str) {
    let source = Path::new("vendor").join(name).join("src");
    let parser = source.join("parser.c");
    let scanner = source.join("scanner.c");
    let mut build = cc::Build::new();
    build
        .std("c11")
        .warnings(false)
        .include(&source)
        .file(&parser);
    if scanner.exists() {
        build.file(&scanner);
        println!("cargo:rerun-if-changed={}", scanner.display());
    }
    build.compile(name);
    println!("cargo:rerun-if-changed={}", parser.display());
}

fn main() {
    if std::env::var_os("CARGO_FEATURE_LANG_SASS").is_some() {
        compile("tree-sitter-sass");
    }
    if std::env::var_os("CARGO_FEATURE_LANG_MDX").is_some() {
        compile("tree-sitter-mdx");
    }
}
