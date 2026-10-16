//! Named accounts (API 5): a unique username per server and a password.
//!
//! The rest of the lobby API is unchanged: logging in returns the same kind
//! of account key anonymous accounts get from `POST /register`, so every
//! other call keeps carrying `key`. An account can hold several keys (one
//! per device); changing the password or LOG OUT EVERYWHERE revokes them.
//!
//! ```text
//! POST /v1/account/register  {username, password}            → Session
//! POST /v1/account/login     {username, password}            → Session (a new key)
//! POST /v1/account/claim     {key, username, password}       → Session (anonymous account
//!                                                               keeps its id, rating, history)
//! GET  /v1/account?key=                                      → AccountInfo
//! POST /v1/account/password  {key, password, new_password}   → Session (fresh key; every
//!                                                               other key is revoked)
//! POST /v1/account/logout    {key}                           this key only
//! POST /v1/account/logout_all {key}                          every key of the account
//! ```
//!
//! There is no e-mail, so there is no self-service recovery: the server's
//! operator can reset a password (`rugen-server --reset-password <user>`).
//! Passwords travel only inside the HTTPS request bodies; clients never
//! store them.

use serde::{Deserialize, Serialize};

use crate::ProfileInfo;

pub const USERNAME_MIN: usize = 3;
pub const USERNAME_MAX: usize = 16;
pub const PASSWORD_MIN: usize = 6;
pub const PASSWORD_MAX: usize = 128;

/// Names nobody may register (compared without case; `PLAYER1234` style
/// names are what anonymous accounts are called).
const RESERVED: &[&str] = &[
    "admin",
    "administrator",
    "rugen",
    "official",
    "server",
    "moderator",
    "mod",
    "staff",
    "system",
    "root",
    "support",
    "guest",
    "anonymous",
    "player",
    "null",
    "nobody",
];

/// Whether `name` is reserved (case-insensitive; also `rugen*`, `official*`,
/// `admin*` and `player` followed by digits).
pub fn is_reserved_username(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    if RESERVED.contains(&n.as_str()) {
        return true;
    }
    if ["rugen", "official", "admin", "moderator"]
        .iter()
        .any(|p| n.starts_with(p))
    {
        return true;
    }
    n.strip_prefix("player")
        .is_some_and(|d| d.bytes().all(|b| b.is_ascii_digit()))
}

/// Why `name` cannot be a username (`None` = fine): 3 to 16 ASCII letters,
/// digits, `_` or `-`, not reserved. Uniqueness (case-insensitive) is the
/// server's to check.
pub fn username_error(name: &str) -> Option<&'static str> {
    let len = name.chars().count();
    if !(USERNAME_MIN..=USERNAME_MAX).contains(&len)
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Some("USERNAME: 3 TO 16 LETTERS, DIGITS, _ OR -");
    }
    if is_reserved_username(name) {
        return Some("THIS USERNAME IS RESERVED");
    }
    None
}

/// Why `password` cannot be used (`None` = fine): 6 to 128 characters, no
/// other rule.
pub fn password_error(password: &str) -> Option<&'static str> {
    let len = password.chars().count();
    if !(PASSWORD_MIN..=PASSWORD_MAX).contains(&len) {
        return Some("PASSWORD: 6 TO 128 CHARACTERS");
    }
    None
}

/// Reply of register / login / claim / password change: the account key
/// this device keeps (never the password).
#[derive(Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Session {
    pub id: u64,
    pub key: String,
    pub username: String,
    pub profile: ProfileInfo,
}

/// Reply of `POST /v1/account/link_code`: a short code the player types on
/// the website to link this account (valid `expires_in` seconds, once).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LinkCode {
    pub code: String,
    /// Unix time it stops working.
    pub expires_at: u64,
    /// Seconds it is good for, from when it was made.
    pub expires_in: u64,
}

// account keys must never end up in logs
impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("id", &self.id)
            .field("key", &"<redacted>")
            .field("username", &self.username)
            .finish()
    }
}

/// `GET /v1/account`: who a key belongs to.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AccountInfo {
    pub id: u64,
    /// Empty for an anonymous account (it can be claimed).
    pub username: String,
    /// Keys (devices) logged in.
    pub sessions: u32,
    pub profile: ProfileInfo,
}

impl AccountInfo {
    pub fn named(&self) -> bool {
        !self.username.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn username_rules() {
        assert_eq!(username_error("axel"), None);
        assert_eq!(username_error("Ax_el-99"), None);
        assert_eq!(username_error("abc"), None);
        assert_eq!(username_error("a234567890123456"), None);
        assert!(username_error("ab").is_some());
        assert!(username_error("a2345678901234567").is_some());
        assert!(username_error("with space").is_some());
        assert!(username_error("émile").is_some());
        assert!(username_error("a.b").is_some());
        assert!(username_error("").is_some());
        for r in [
            "admin",
            "ADMIN",
            "Rugen",
            "official",
            "server",
            "moderator",
            "rugen_dev",
            "Player1234",
            "player",
            "OfficialBot",
        ] {
            assert_eq!(username_error(r), Some("THIS USERNAME IS RESERVED"), "{r}");
        }
        // names merely containing a reserved word are fine
        assert_eq!(username_error("player_one"), None);
        assert_eq!(username_error("superadmin"), None);
    }

    #[test]
    fn password_rules() {
        assert!(password_error("12345").is_some());
        assert_eq!(password_error("123456"), None);
        assert_eq!(password_error("aaaaaa"), None, "no complexity rules");
        assert_eq!(password_error(&"x".repeat(128)), None);
        assert!(password_error(&"x".repeat(129)).is_some());
        assert_eq!(password_error("ñandú!"), None, "characters, not bytes");
    }

    #[test]
    fn sessions_round_trip_and_never_debug_the_key() {
        let s = Session {
            id: 7,
            key: "k".repeat(64),
            username: "axel".into(),
            profile: ProfileInfo {
                id: 7,
                name: "axel".into(),
                verified: true,
                ..Default::default()
            },
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
        assert!(!format!("{s:?}").contains(&s.key));
        let info: AccountInfo = serde_json::from_str(r#"{"id":3}"#).unwrap();
        assert!(!info.named());
    }
}
