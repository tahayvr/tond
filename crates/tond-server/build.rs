// sqlx::migrate!("./migrations") embeds the migration files at compile time
// but doesn't tell cargo to watch them: without this, an incremental build
// keeps serving migrations that were edited, folded or deleted.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
