//! The admin password: choosing it at start-up, checking it, changing it.

use argon2::Argon2;
use argon2::password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash};
use vigil_store::{Store, StoreError};

/// The setting that holds the password's hash.
const HASH_SETTING: &str = "admin_password_hash";

/// Characters of a generated password: no look-alikes (`0`/`O`, `1`/`l`/`I`).
const ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// Length of a generated password, about 138 bits.
const GENERATED_LEN: usize = 24;

/// Why the admin password could not be set up or checked.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// The database failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Hashing failed.
    #[error("hashing the password failed: {0}")]
    Hash(String),
    /// The system has no source of random numbers.
    #[error("no source of random numbers: {0}")]
    Random(String),
    /// The password hashing task did not finish.
    #[error("the password check did not finish")]
    Task,
}

/// Where the admin password in force came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasswordSource {
    /// `VIGIL_ADMIN_PASSWORD` is set; it is the password and wins over any other.
    Environment,
    /// A password saved earlier (from a previous start or from the settings page) is kept.
    Stored,
    /// There was none, so this one was made up. It is shown once; only its hash is kept.
    Generated(String),
}

/// Make sure there is an admin password to sign in with, and say where it came from.
///
/// With `from_env` (an empty value counts as unset) that is the password, and its hash is
/// stored unless the stored one already matches. Otherwise a password saved earlier stays,
/// and when there is none a random one is made, stored as a hash and returned so that the
/// caller can show it once.
pub async fn ensure_admin_password(
    store: &Store,
    from_env: Option<&str>,
) -> Result<PasswordSource, AuthError> {
    let stored = store.get_setting(HASH_SETTING).await?;
    if let Some(password) = from_env.filter(|password| !password.is_empty()) {
        let unchanged = match &stored {
            Some(hash) => check(password, hash).await?,
            None => false,
        };
        if !unchanged {
            store
                .set_setting(HASH_SETTING, &hash(password).await?)
                .await?;
        }
        return Ok(PasswordSource::Environment);
    }
    if stored.is_some() {
        return Ok(PasswordSource::Stored);
    }
    let password = generate()?;
    store
        .set_setting(HASH_SETTING, &hash(&password).await?)
        .await?;
    Ok(PasswordSource::Generated(password))
}

/// Whether `password` is the admin password. False when none is set.
pub(crate) async fn verify_admin_password(
    store: &Store,
    password: &str,
) -> Result<bool, AuthError> {
    match store.get_setting(HASH_SETTING).await? {
        Some(stored) => check(password, &stored).await,
        None => Ok(false),
    }
}

/// Replace the admin password.
pub async fn change_password(store: &Store, new_password: &str) -> Result<(), AuthError> {
    store
        .set_setting(HASH_SETTING, &hash(new_password).await?)
        .await?;
    Ok(())
}

/// Hash a password into a PHC string. Runs on a blocking thread: it takes tens of
/// milliseconds of CPU and 19 MiB of memory by design.
async fn hash(password: &str) -> Result<String, AuthError> {
    let password = password.to_owned();
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(password.as_bytes())
            .map(|hash| hash.to_string())
            .map_err(|error| AuthError::Hash(error.to_string()))
    })
    .await
    .map_err(|_| AuthError::Task)?
}

/// Whether `password` matches a stored hash. A hash that cannot be read counts as no match.
async fn check(password: &str, stored: &str) -> Result<bool, AuthError> {
    let (password, stored) = (password.to_owned(), stored.to_owned());
    tokio::task::spawn_blocking(move || {
        let Ok(parsed) = PasswordHash::new(&stored) else {
            return false;
        };
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    })
    .await
    .map_err(|_| AuthError::Task)
}

fn generate() -> Result<String, AuthError> {
    let mut bytes = [0u8; GENERATED_LEN];
    getrandom::fill(&mut bytes).map_err(|error| AuthError::Random(error.to_string()))?;
    // 57 characters: a byte's remainder is only slightly biased, which is of no use to anyone
    // guessing 24 of them.
    Ok(bytes
        .iter()
        .map(|byte| char::from(ALPHABET[usize::from(*byte) % ALPHABET.len()]))
        .collect())
}
