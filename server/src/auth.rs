use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use crate::config::{ServerConfig, verify_password};

pub struct Sessions {
    inner: Mutex<HashMap<String, Instant>>,
    ttl: Duration,
}

impl Sessions {
    pub fn new(hours: u64) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            ttl: Duration::from_secs(hours.saturating_mul(3600).max(3600)),
        }
    }

    pub fn create(&self) -> String {
        let token = crate::config::random_token();
        self.inner
            .lock()
            .unwrap()
            .insert(token.clone(), Instant::now());
        token
    }

    pub fn valid(&self, token: &str) -> bool {
        let mut map = self.inner.lock().unwrap();
        map.retain(|_, at| at.elapsed() < self.ttl);
        map.contains_key(token)
    }
}

pub fn agent_authorized(cfg: &ServerConfig, token: &str) -> bool {
    use subtle::ConstantTimeEq;
    cfg.agent_tokens.iter().any(|t| {
        let a = t.as_bytes();
        let b = token.as_bytes();
        a.len() == b.len() && bool::from(a.ct_eq(b))
    })
}

pub fn admin_login(cfg: &ServerConfig, sessions: &Sessions, password: &str) -> Option<String> {
    if verify_password(password, &cfg.admin_password_hash) {
        Some(sessions.create())
    } else {
        None
    }
}
