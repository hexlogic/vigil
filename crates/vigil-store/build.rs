//! Re-run the build when migrations change so `sqlx::migrate!` embeds the latest set.

fn main() {
    println!("cargo:rerun-if-changed=../../migrations");
}
