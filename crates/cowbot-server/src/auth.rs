//! Accounts, sessions, and sign-in rate limiting.
//!
//! Passwords are hashed with Argon2id (ADR-0043). Session tokens are random,
//! stored only as a SHA-256 hash, and presented as a bearer token; the server
//! never keeps the token itself, so a database read cannot mint a session.
//!
//! The local runtime hashes nothing — a local workspace is one trusted user on
//! one machine. The server is where authentication starts, and it does not
//! reuse the empty `password_hash` a local bootstrap leaves.

use std::collections::HashMap;
use std::sync::Mutex;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use chrono::{DateTime, Duration, Utc};
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::identity::{Membership, Role, Session, User, UserId, Workspace};
use cowbot_store::Store;
use sha2::{Digest, Sha256};

use crate::config::ServerConfig;

/// The signed-in identity a request carries.
#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    pub user: User,
    pub session_id: String,
    pub expires_at: DateTime<Utc>,
}

/// A newly minted session: the row is stored, and the token is returned once.
#[derive(Debug, Clone)]
pub struct IssuedSession {
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

/// Password and session operations over the server's store.
pub struct AuthService {
    store: Store,
    config: ServerConfig,
    /// Failed sign-ins per source, with the time of the first failure in the
    /// current window.
    attempts: Mutex<HashMap<String, (u32, DateTime<Utc>)>>,
}

impl AuthService {
    pub fn new(store: Store, config: ServerConfig) -> Self {
        Self {
            store,
            config,
            attempts: Mutex::new(HashMap::new()),
        }
    }

    /// Create the owner account and workspace on an empty database.
    ///
    /// This is the server counterpart of the local bootstrap: a fresh
    /// deployment needs one way in, and that credential is printed once for the
    /// operator to change. A database that already has users is left alone.
    pub async fn bootstrap(&self, owner_password: &str) -> CowResult<Option<User>> {
        if !self.store.list_users().await?.is_empty() {
            return Ok(None);
        }
        let hash = hash_password(owner_password)?;
        let user = User {
            password_hash: hash,
            ..User::new(&self.config.owner_name, &self.config.owner_email, "")
        };
        self.store.create_user(&user).await?;
        let workspace = Workspace::new(&self.config.workspace_name);
        self.store.create_workspace(&workspace).await?;
        self.store
            .add_member(&Membership::new(user.id.clone(), workspace.id, Role::Owner))
            .await?;
        Ok(Some(user))
    }

    /// Sign a user in and issue a session token.
    ///
    /// The same error is returned for an unknown email and a wrong password, so
    /// the endpoint cannot be used to discover which accounts exist.
    pub async fn sign_in(
        &self,
        email: &str,
        password: &str,
        source: &str,
    ) -> CowResult<IssuedSession> {
        if self.rate_limited(source) {
            return Err(CowError::Config(
                "too many sign-in attempts; wait a moment and try again".into(),
            ));
        }
        let user = self.store.get_user_by_email(email).await?;
        let verified = match &user {
            Some(user) => verify_password(password, &user.password_hash).unwrap_or(false),
            None => {
                // Spend the same work on an unknown email as on a real one, so
                // timing does not reveal which is which.
                let _ = verify_password(password, DUMMY_HASH);
                false
            }
        };
        if !verified {
            self.record_failure(source);
            return Err(CowError::Config(
                "the email or password is not correct".into(),
            ));
        }
        let user = user.expect("a verified password implies a user");
        self.clear_failures(source);
        self.issue(&user.id).await
    }

    /// Issue a fresh session for a known user.
    pub async fn issue(&self, user_id: &UserId) -> CowResult<IssuedSession> {
        let token = random_token();
        let expires_at = Utc::now() + Duration::hours(self.config.session_hours);
        let session = Session::new(user_id.clone(), hash_token(&token), expires_at);
        self.store.create_session(&session).await?;
        Ok(IssuedSession { token, expires_at })
    }

    /// Resolve a bearer token to a signed-in user.
    ///
    /// An expired session is deleted on the way out, so a stale token is
    /// cleaned up rather than accumulating.
    pub async fn authenticate(&self, token: &str) -> CowResult<Option<AuthenticatedUser>> {
        let Some(session) = self
            .store
            .get_session_by_token_hash(&hash_token(token))
            .await?
        else {
            return Ok(None);
        };
        if session.expires_at <= Utc::now() {
            self.store.delete_session(&session.id).await?;
            return Ok(None);
        }
        let Some(user) = self.store.get_user(&session.user_id).await? else {
            return Ok(None);
        };
        Ok(Some(AuthenticatedUser {
            user,
            session_id: session.id,
            expires_at: session.expires_at,
        }))
    }

    /// End one session.
    pub async fn sign_out(&self, session_id: &str) -> CowResult<()> {
        self.store.delete_session(session_id).await
    }

    /// Change a user's password. Every other session for that user is dropped,
    /// so a stolen token cannot outlive the change.
    pub async fn change_password(
        &self,
        user: &User,
        current_password: &str,
        new_password: &str,
    ) -> CowResult<()> {
        if !verify_password(current_password, &user.password_hash).unwrap_or(false) {
            return Err(CowError::Config(
                "the current password is not correct".into(),
            ));
        }
        if new_password.chars().count() < MIN_PASSWORD_CHARS {
            return Err(CowError::Config(format!(
                "the new password must be at least {MIN_PASSWORD_CHARS} characters"
            )));
        }
        let hash = hash_password(new_password)?;
        self.store.update_password(&user.id, &hash).await?;
        // The caller re-issues a session; every older token dies here.
        self.store.delete_user_sessions(&user.id).await?;
        Ok(())
    }

    /// The workspaces and roles a user belongs to.
    pub async fn memberships(&self, user_id: &UserId) -> CowResult<Vec<Membership>> {
        self.store.list_memberships(user_id).await
    }

    fn rate_limited(&self, source: &str) -> bool {
        let Ok(attempts) = self.attempts.lock() else {
            return false;
        };
        let Some((count, first)) = attempts.get(source) else {
            return false;
        };
        let window = Duration::seconds(self.config.login_window_secs);
        *first + window > Utc::now() && *count >= self.config.login_attempts
    }

    fn record_failure(&self, source: &str) {
        let Ok(mut attempts) = self.attempts.lock() else {
            return;
        };
        let window = Duration::seconds(self.config.login_window_secs);
        let now = Utc::now();
        let entry = attempts.entry(source.to_string()).or_insert((0, now));
        if entry.1 + window <= now {
            // The previous window expired, so this failure starts a new one.
            *entry = (1, now);
        } else {
            entry.0 = entry.0.saturating_add(1);
        }
    }

    fn clear_failures(&self, source: &str) {
        if let Ok(mut attempts) = self.attempts.lock() {
            attempts.remove(source);
        }
    }
}

/// Shortest password the server accepts.
pub const MIN_PASSWORD_CHARS: usize = 10;

/// A fixed hash used to spend comparable work on an unknown email.
const DUMMY_HASH: &str =
    "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHR2YWx1ZQ$YjW0eGJqS0lwV2J4RG5jWXl0U0JqR3BtV0x4c3JhYkE";

fn hash_password(password: &str) -> CowResult<String> {
    if password.chars().count() < MIN_PASSWORD_CHARS {
        return Err(CowError::Config(format!(
            "the password must be at least {MIN_PASSWORD_CHARS} characters"
        )));
    }
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| CowError::Config(format!("the password could not be hashed: {error}")))
}

fn verify_password(password: &str, hash: &str) -> Result<bool, argon2::password_hash::Error> {
    let parsed = PasswordHash::new(hash)?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

/// A 256-bit random token, hex-encoded.
fn random_token() -> String {
    use argon2::password_hash::rand_core::RngCore;
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The stored form of a session token.
fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
