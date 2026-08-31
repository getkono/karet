include!("tests/documents.rs");
include!("tests/formats.rs");
include!("tests/missing_documents.rs");
include!("tests/latex.rs");
include!("tests/vcs.rs");
#[cfg(feature = "aicommit")]
include!("tests/aicommit.rs");
include!("tests/persistence.rs");
include!("tests/search.rs");
include!("tests/spelling.rs");
include!("tests/lsp_updates.rs");
include!("tests/seam.rs");
