//! Master password login and the sessions it creates.
//!
//! Sessions live in memory only, so a restart logs everyone out.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use subtle::ConstantTimeEq;

/// Failed logins are answered this late, and one at a time, which limits password guessing
/// to about one attempt per second.
const FAILED_LOGIN_DELAY: Duration = Duration::from_secs(1);
const TOKEN_BYTES: usize = 32;

pub struct Authenticator {
    password: Box<str>,
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
    pub fn new(password: &str, session_lifetime: Duration) -> Self {
        Self {
            password: password.into(),
            session_lifetime,
            sessions: Mutex::new(HashMap::new()),
            login_attempts: tokio::sync::Mutex::new(()),
        }
    }

    /// Starts a session if `password` is the master password.
    pub async fn login(&self, password: &str) -> Option<Session> {
        let _one_at_a_time = self.login_attempts.lock().await;
        if !bool::from(password.as_bytes().ct_eq(self.password.as_bytes())) {
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

    #[tokio::test]
    async fn the_master_password_starts_a_session() {
        let auth = Authenticator::new("secret", Duration::from_secs(60));

        let session = auth.login("secret").await.unwrap();

        assert_eq!(session.token.len(), TOKEN_BYTES * 2);
        assert!(auth.is_valid(&session.token));
        assert!(!auth.is_valid("guessed"));
    }

    #[tokio::test(start_paused = true)]
    async fn wrong_passwords_are_rejected_after_a_delay() {
        let auth = Authenticator::new("secret", Duration::from_secs(60));
        let started = tokio::time::Instant::now();

        assert!(auth.login("Secret").await.is_none());
        assert!(auth.login("").await.is_none());
        assert!(started.elapsed() >= FAILED_LOGIN_DELAY * 2);
    }

    #[tokio::test]
    async fn logout_and_expiry_end_the_session() {
        let auth = Authenticator::new("secret", Duration::ZERO);
        let expired = auth.login("secret").await.unwrap();
        assert!(!auth.is_valid(&expired.token));

        let auth = Authenticator::new("secret", Duration::from_secs(60));
        let session = auth.login("secret").await.unwrap();
        auth.logout(&session.token);
        assert!(!auth.is_valid(&session.token));
    }
}
