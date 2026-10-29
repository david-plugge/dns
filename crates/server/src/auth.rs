//! Master password login and the sessions it creates.
//!
//! Sessions live in memory only, so a restart logs everyone out.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use argon2::{Argon2, PasswordVerifier, password_hash::phc::PasswordHash};
use subtle::ConstantTimeEq;

/// Failed logins are answered this late, and one at a time, which limits password guessing
/// to about one attempt per second.
const FAILED_LOGIN_DELAY: Duration = Duration::from_secs(1);
const TOKEN_BYTES: usize = 32;

/// The master password, as given in the configuration.
#[derive(Debug, Clone)]
pub enum MasterPassword {
    Plain(Box<str>),
    /// An Argon2id hash, so deployment files need not contain the password itself.
    Argon2id(Box<PasswordHash>),
}

impl MasterPassword {
    /// Accepts only Argon2id hashes in PHC format, e.g. `$argon2id$v=19$m=…`.
    pub fn parse_argon2id(text: &str) -> Result<Self, String> {
        let hash = PasswordHash::new(text)
            .map_err(|error| format!("not a password hash in PHC format: {error}"))?;
        if hash.algorithm != argon2::Algorithm::Argon2id.ident() {
            return Err(format!("expected an argon2id hash, got {}", hash.algorithm));
        }
        Ok(Self::Argon2id(Box::new(hash)))
    }

    /// Hashing is deliberately slow, so callers on the async runtime should not block on it.
    fn matches(&self, password: &str) -> bool {
        match self {
            MasterPassword::Plain(expected) => {
                password.as_bytes().ct_eq(expected.as_bytes()).into()
            }
            MasterPassword::Argon2id(hash) => Argon2::default()
                .verify_password(password.as_bytes(), &**hash)
                .is_ok(),
        }
    }
}

pub struct Authenticator {
    password: MasterPassword,
    session_lifetime: Duration,
    /// Session token to when it was created; comparing ages cannot overflow like adding
    /// a long lifetime to now could.
    sessions: Mutex<HashMap<String, Instant>>,
    login_attempts: tokio::sync::Mutex<()>,
}

pub struct Session {
    pub token: String,
    pub lifetime: Duration,
}

impl Authenticator {
    pub fn new(password: MasterPassword, session_lifetime: Duration) -> Self {
        Self {
            password,
            session_lifetime,
            sessions: Mutex::new(HashMap::new()),
            login_attempts: tokio::sync::Mutex::new(()),
        }
    }

    /// Starts a session if `password` is the master password.
    pub async fn login(&self, password: &str) -> Option<Session> {
        let _one_at_a_time = self.login_attempts.lock().await;
        let expected = self.password.clone();
        let attempt = password.to_owned();
        let matches = tokio::task::spawn_blocking(move || expected.matches(&attempt))
            .await
            .unwrap_or(false);
        if !matches {
            tokio::time::sleep(FAILED_LOGIN_DELAY).await;
            return None;
        }

        let token = new_token();
        let mut sessions = self.sessions.lock().expect("session lock poisoned");
        sessions.retain(|_, created_at| created_at.elapsed() < self.session_lifetime);
        sessions.insert(token.clone(), Instant::now());
        Some(Session {
            token,
            lifetime: self.session_lifetime,
        })
    }

    pub fn is_valid(&self, token: &str) -> bool {
        let sessions = self.sessions.lock().expect("session lock poisoned");
        sessions
            .get(token)
            .is_some_and(|created_at| created_at.elapsed() < self.session_lifetime)
    }

    pub fn logout(&self, token: &str) {
        let mut sessions = self.sessions.lock().expect("session lock poisoned");
        sessions.remove(token);
    }
}

fn new_token() -> String {
    let mut bytes = [0; TOKEN_BYTES];
    getrandom::fill(&mut bytes).expect("the operating system provides no randomness");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "secret", hashed with deliberately cheap parameters so the test stays fast.
    const SECRET_HASH: &str = "$argon2id$v=19$m=64,t=1,p=1$SAVx/efSrm2ZlPSv8Z8t9w$wnr64EUDg64wy7VVK5RN48tk5bXeFYiMTMY5lu1PVuA";

    fn plain(password: &str) -> MasterPassword {
        MasterPassword::Plain(password.into())
    }

    #[tokio::test]
    async fn an_argon2id_hash_accepts_only_its_password() {
        let auth = Authenticator::new(
            MasterPassword::parse_argon2id(SECRET_HASH).unwrap(),
            Duration::from_secs(60),
        );

        assert!(auth.login("secret").await.is_some());
        assert!(auth.login("Secret").await.is_none());
    }

    #[test]
    fn only_argon2id_hashes_are_accepted() {
        let argon2i = "$argon2i$v=19$m=64,t=1,p=1$9yslu/cD3ZtwBYKJpnIUbw$4eR3WsgKb0t9K4UZ92BsBx4rV/Lnz3FSnSXjFVFVHq0";

        assert!(MasterPassword::parse_argon2id(argon2i).is_err());
        assert!(MasterPassword::parse_argon2id("secret").is_err());
    }

    #[tokio::test]
    async fn the_master_password_starts_a_session() {
        let auth = Authenticator::new(plain("secret"), Duration::from_secs(60));

        let session = auth.login("secret").await.unwrap();

        assert_eq!(session.token.len(), TOKEN_BYTES * 2);
        assert!(auth.is_valid(&session.token));
        assert!(!auth.is_valid("guessed"));
    }

    #[tokio::test(start_paused = true)]
    async fn wrong_passwords_are_rejected_after_a_delay() {
        let auth = Authenticator::new(plain("secret"), Duration::from_secs(60));
        let started = tokio::time::Instant::now();

        assert!(auth.login("Secret").await.is_none());
        assert!(auth.login("").await.is_none());
        assert!(started.elapsed() >= FAILED_LOGIN_DELAY * 2);
    }

    #[tokio::test]
    async fn logout_and_expiry_end_the_session() {
        let auth = Authenticator::new(plain("secret"), Duration::ZERO);
        let expired = auth.login("secret").await.unwrap();
        assert!(!auth.is_valid(&expired.token));

        let auth = Authenticator::new(plain("secret"), Duration::from_secs(60));
        let session = auth.login("secret").await.unwrap();
        auth.logout(&session.token);
        assert!(!auth.is_valid(&session.token));
    }
}
