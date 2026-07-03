//! Print the JSON Schema for karet's `Settings` to stdout.
//!
//! Regenerate the checked-in schema with:
//!
//! ```sh
//! cargo run -p karet-session --example settings-schema > settings.schema.json
//! ```
//!
//! A test in `karet-session` (`config::tests::checked_in_schema_is_current`) fails if
//! the committed file drifts from this output.

fn main() {
    println!("{}", karet_session::config::json_schema());
}
