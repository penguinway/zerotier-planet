use std::time::{SystemTime, UNIX_EPOCH};

use rand_core::{OsRng, RngCore};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use crate::error::{AppError, AppResult};
use crate::store::{Role, User};

pub const SESSION_TTL_SECS: u64 = 7 * 24 * 3600;

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    OsRng.fill_bytes(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

pub fn hash_password(password: &str) -> AppResult<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AppError::internal(format!("密码哈希失败: {e}")))
}

pub fn verify_password(user: &User, password: &str) -> bool {
    PasswordHash::new(&user.password_hash)
        .map(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
        .unwrap_or(false)
}

pub fn check_password_strength(password: &str) -> AppResult<()> {
    if password.chars().count() < 6 {
        return Err(AppError::bad_request("密码至少 6 位"));
    }
    Ok(())
}

pub fn check_username(name: &str) -> AppResult<()> {
    let ok = (2..=32).contains(&name.len())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if !ok {
        return Err(AppError::bad_request("用户名只能包含字母、数字、_ . -，长度 2-32"));
    }
    Ok(())
}

pub fn new_user(password: &str, role: Role, must_change: bool) -> AppResult<User> {
    check_password_strength(password)?;
    Ok(User {
        password_hash: hash_password(password)?,
        role,
        must_change,
        token_version: 0,
        created: now_secs(),
    })
}

pub fn set_password(user: &mut User, password: &str, must_change: bool) -> AppResult<()> {
    check_password_strength(password)?;
    user.password_hash = hash_password(password)?;
    user.must_change = must_change;
    user.token_version += 1;
    Ok(())
}

fn sign(secret: &str, payload: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key");
    mac.update(payload.as_bytes());
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

/// 会话令牌：base64(用户名).过期时间.令牌版本.签名
pub fn create_token(secret: &str, username: &str, user: &User) -> String {
    let payload = format!(
        "{}.{}.{}",
        URL_SAFE_NO_PAD.encode(username),
        now_secs() + SESSION_TTL_SECS,
        user.token_version
    );
    let sig = sign(secret, &payload);
    format!("{payload}.{sig}")
}

/// 校验令牌，返回用户名。签名、过期时间、用户存在性、令牌版本都要通过
pub fn verify_token<'a>(
    secret: &str,
    token: &str,
    lookup: impl Fn(&str) -> Option<&'a User>,
) -> Option<String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let payload = parts[..3].join(".");
    if !ct_eq(&sign(secret, &payload), parts[3]) {
        return None;
    }
    let username = String::from_utf8(URL_SAFE_NO_PAD.decode(parts[0]).ok()?).ok()?;
    let exp: u64 = parts[1].parse().ok()?;
    let version: u64 = parts[2].parse().ok()?;
    let user = lookup(&username)?;
    if exp < now_secs() || version != user.token_version {
        return None;
    }
    Some(username)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_roundtrip_and_invalidation() {
        let mut user = new_user("secret1", Role::User, false).unwrap();
        let token = create_token("k", "alice", &user);
        assert_eq!(verify_token("k", &token, |_| Some(&user)), Some("alice".into()));
        assert_eq!(verify_token("other", &token, |_| Some(&user)), None);
        assert_eq!(verify_token("k", &token, |_| None), None);
        set_password(&mut user, "secret2", false).unwrap();
        assert_eq!(verify_token("k", &token, |_| Some(&user)), None);
        assert!(verify_password(&user, "secret2"));
        assert!(!verify_password(&user, "secret1"));
    }

    #[test]
    fn username_rules() {
        assert!(check_username("bob.lab-1").is_ok());
        assert!(check_username("a").is_err());
        assert!(check_username("bad name").is_err());
        assert!(check_username("../x").is_err());
    }
}
