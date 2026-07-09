// Usage: cargo run --example reset_password --features web -- <data-dir> <new-password>
fn main() -> anyhow::Result<()> {
    let dir = std::env::args().nth(1).expect("usage: reset_password <data-dir> <password>");
    let pw = std::env::args().nth(2).expect("usage: reset_password <data-dir> <password>");
    let duck_path = std::path::PathBuf::from(&dir).join("fit-dashboard.duckdb");

    use argon2::{password_hash::{rand_core::OsRng, PasswordHasher, SaltString}, Argon2};
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default().hash_password(pw.as_bytes(), &salt).map_err(|e| anyhow::anyhow!("{e}"))?.to_string();

    let conn = duckdb::Connection::open(&duck_path)?;
    let updated = conn.execute("UPDATE users SET password_hash = ?1", duckdb::params![hash])?;
    println!("updated {} row(s) — password is now: {}", updated, pw);
    Ok(())
}
