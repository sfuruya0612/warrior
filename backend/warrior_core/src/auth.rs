//! Google OAuth とセッションの純粋な検証。

use std::fmt;

use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::base64url;

/// OAuth の state の有効期限 (秒)。
pub const OAUTH_STATE_TTL_SECONDS: i64 = 5 * 60;
/// セッションの有効期限 (秒)。
pub const SESSION_TTL_SECONDS: i64 = 30 * 24 * 60 * 60;
/// WebAuthn のチャレンジと再認証の有効期限 (秒)。
pub const WEBAUTHN_CHALLENGE_TTL_SECONDS: i64 = 5 * 60;
/// 再認証の短命トークンの有効期限 (秒)。
pub const ELEVATION_TTL_SECONDS: i64 = 5 * 60;
/// セッションの Cookie の名前。
pub const SESSION_COOKIE: &str = "warrior_session";
/// 端末トークンの Cookie の名前。
pub const TERMINAL_COOKIE: &str = "warrior_terminal";
/// 端末トークンの有効期限 (秒)。
pub const TERMINAL_TTL_SECONDS: i64 = 365 * 24 * 60 * 60;
/// 再認証の短命トークンの Cookie の名前。
pub const ELEVATION_COOKIE: &str = "warrior_elevation";
/// OAuth の state の Cookie の名前。
pub const OAUTH_STATE_COOKIE: &str = "warrior_oauth_state";

/// 認証の検証の誤り。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// JWT の形が違う。
    InvalidTokenFormat,
    /// payload が JSON として読めない。
    InvalidPayload,
    /// `aud` が一致しない。
    InvalidAudience,
    /// `iss` が一致しない。
    InvalidIssuer,
    /// 有効期限を過ぎている。
    Expired,
    /// `nonce` が一致しない。
    InvalidNonce,
    /// `sub` が無い、または一致しない。
    InvalidSubject,
    /// メールアドレスが確認されていない。
    EmailNotVerified,
    /// メールアドレスが無い。
    InvalidEmail,
    /// 表示名が無い。
    InvalidName,
}

impl fmt::Display for AuthError {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        match self {
            AuthError::InvalidTokenFormat => write!(formatter, "invalid ID token format"),
            AuthError::InvalidPayload => write!(formatter, "invalid ID token payload"),
            AuthError::InvalidAudience => write!(formatter, "invalid audience"),
            AuthError::InvalidIssuer => write!(formatter, "invalid issuer"),
            AuthError::Expired => write!(formatter, "expired"),
            AuthError::InvalidNonce => write!(formatter, "invalid nonce"),
            AuthError::InvalidSubject => write!(formatter, "invalid subject"),
            AuthError::EmailNotVerified => write!(formatter, "email is not verified"),
            AuthError::InvalidEmail => write!(formatter, "invalid email"),
            AuthError::InvalidName => write!(formatter, "invalid name"),
        }
    }
}

impl std::error::Error for AuthError {}

/// 32 バイトの乱数から作る state などの値を受け付けるかを判定する。
pub fn is_valid_opaque_token(token: &str) -> bool {
    token.len() == 43
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// 入力の SHA-256 を base64url で返す。
pub fn sha256_base64url(input: &[u8]) -> String {
    let digest = Sha256::digest(input);
    base64url::encode(&digest)
}

/// セッションの値を D1 に保存するハッシュにする。
pub fn hash_session_token(token: &str) -> String {
    sha256_base64url(token.as_bytes())
}

/// 端末トークンの値を D1 に保存するハッシュにする。
///
/// 端末トークンは 32 バイトの乱数であり、D1 には SHA-256 の小文字 16 進だけを保存する。
pub fn hash_terminal_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// ID トークンの payload をデコードする。署名は検証しない (ADR-019)。
pub fn decode_id_token_payload(id_token: &str) -> Result<Value, AuthError> {
    let mut parts = id_token.split('.');
    let _header = parts.next().ok_or(AuthError::InvalidTokenFormat)?;
    let payload = parts.next().ok_or(AuthError::InvalidTokenFormat)?;
    let _signature = parts.next().ok_or(AuthError::InvalidTokenFormat)?;
    if parts.next().is_some() {
        return Err(AuthError::InvalidTokenFormat);
    }
    let bytes = base64url::decode(payload).ok_or(AuthError::InvalidPayload)?;
    serde_json::from_slice(&bytes).map_err(|_| AuthError::InvalidPayload)
}

/// ID トークンの payload から取り出すクレーム。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdTokenClaims {
    /// 利用者の識別子。
    pub sub: String,
    /// メールアドレス。
    pub email: Option<String>,
}

/// ID トークンの payload を検証する。
///
/// `aud`、`iss`、`exp`、`nonce` を検証する。署名は検証しない (ADR-019)。
pub fn validate_id_token(
    id_token: &str,
    expected_audience: &str,
    expected_issuer: &str,
    expected_nonce: &str,
    now_epoch_seconds: i64,
) -> Result<IdTokenClaims, AuthError> {
    let payload = decode_id_token_payload(id_token)?;
    let audience_ok = match payload.get("aud") {
        Some(Value::String(audience)) => audience == expected_audience,
        Some(Value::Array(audiences)) => audiences
            .iter()
            .any(|audience| audience.as_str() == Some(expected_audience)),
        _ => false,
    };
    if !audience_ok {
        return Err(AuthError::InvalidAudience);
    }
    let issuer = payload.get("iss").and_then(Value::as_str);
    let issuer_ok = issuer == Some(expected_issuer)
        || (expected_issuer == "https://accounts.google.com"
            && issuer == Some("accounts.google.com"));
    if !issuer_ok {
        return Err(AuthError::InvalidIssuer);
    }
    if payload
        .get("exp")
        .and_then(Value::as_i64)
        .is_none_or(|expiry| expiry <= now_epoch_seconds)
    {
        return Err(AuthError::Expired);
    }
    if payload.get("nonce").and_then(Value::as_str) != Some(expected_nonce) {
        return Err(AuthError::InvalidNonce);
    }
    let sub = payload
        .get("sub")
        .and_then(Value::as_str)
        .filter(|sub| !sub.is_empty())
        .ok_or(AuthError::InvalidSubject)?
        .to_owned();
    let email = payload
        .get("email")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(IdTokenClaims { sub, email })
}

/// UserInfo の応答。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct UserInfo {
    /// 利用者の識別子。
    pub sub: String,
    /// メールアドレス。
    pub email: String,
    /// メールアドレスが確認済みか。
    pub email_verified: bool,
    /// 表示名。
    pub name: String,
}

/// UserInfo を検証する。
pub fn validate_userinfo(value: &Value, expected_sub: &str) -> Result<UserInfo, AuthError> {
    let userinfo: UserInfo =
        serde_json::from_value(value.clone()).map_err(|_| AuthError::InvalidPayload)?;
    if userinfo.sub != expected_sub {
        return Err(AuthError::InvalidSubject);
    }
    if !userinfo.email_verified {
        return Err(AuthError::EmailNotVerified);
    }
    if userinfo.email.trim().is_empty() {
        return Err(AuthError::InvalidEmail);
    }
    if userinfo.name.trim().is_empty() {
        return Err(AuthError::InvalidName);
    }
    Ok(userinfo)
}

/// セッションの Cookie。
pub fn session_cookie(token: &str) -> String {
    session_cookie_with(token, true)
}

/// セッションの Cookie。HTTPS 以外のローカル開発では Secure を付けない。
pub fn session_cookie_with(token: &str, secure: bool) -> String {
    format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly{}; SameSite=Lax; Max-Age={SESSION_TTL_SECONDS}",
        cookie_security(secure)
    )
}

/// セッションを消す Cookie。
pub fn clear_session_cookie() -> String {
    clear_session_cookie_with(true)
}

/// セッションを消す Cookie。HTTPS 以外のローカル開発では Secure を付けない。
pub fn clear_session_cookie_with(secure: bool) -> String {
    format!(
        "{SESSION_COOKIE}=; Path=/; HttpOnly{}; SameSite=Lax; Max-Age=0",
        cookie_security(secure)
    )
}

/// 端末トークンの Cookie。
pub fn terminal_cookie(token: &str) -> String {
    terminal_cookie_with(token, true)
}

/// 端末トークンの Cookie。HTTPS 以外のローカル開発では Secure を付けない。
pub fn terminal_cookie_with(token: &str, secure: bool) -> String {
    format!(
        "{TERMINAL_COOKIE}={token}; Path=/; HttpOnly{}; SameSite=Lax; Max-Age={TERMINAL_TTL_SECONDS}",
        cookie_security(secure)
    )
}

/// 端末トークンを消す Cookie。
pub fn clear_terminal_cookie() -> String {
    clear_terminal_cookie_with(true)
}

/// 端末トークンを消す Cookie。HTTPS 以外のローカル開発では Secure を付けない。
pub fn clear_terminal_cookie_with(secure: bool) -> String {
    format!(
        "{TERMINAL_COOKIE}=; Path=/; HttpOnly{}; SameSite=Lax; Max-Age=0",
        cookie_security(secure)
    )
}

/// OAuth の state の Cookie。
pub fn oauth_state_cookie(state: &str) -> String {
    oauth_state_cookie_with(state, true)
}

/// OAuth の state の Cookie。HTTPS 以外のローカル開発では Secure を付けない。
pub fn oauth_state_cookie_with(state: &str, secure: bool) -> String {
    format!(
        "{OAUTH_STATE_COOKIE}={state}; Path=/; HttpOnly{}; SameSite=Lax; Max-Age={OAUTH_STATE_TTL_SECONDS}",
        cookie_security(secure)
    )
}

/// OAuth の state を消す Cookie。
pub fn clear_oauth_state_cookie() -> String {
    clear_oauth_state_cookie_with(true)
}

/// OAuth の state を消す Cookie。HTTPS 以外のローカル開発では Secure を付けない。
pub fn clear_oauth_state_cookie_with(secure: bool) -> String {
    format!(
        "{OAUTH_STATE_COOKIE}=; Path=/; HttpOnly{}; SameSite=Lax; Max-Age=0",
        cookie_security(secure)
    )
}

/// 再認証の短命トークンの Cookie。
pub fn elevation_cookie(token: &str) -> String {
    elevation_cookie_with(token, true)
}

/// 再認証の短命トークンの Cookie。HTTPS 以外のローカル開発では Secure を付けない。
pub fn elevation_cookie_with(token: &str, secure: bool) -> String {
    format!(
        "{ELEVATION_COOKIE}={token}; Path=/; HttpOnly{}; SameSite=Lax; Max-Age={ELEVATION_TTL_SECONDS}",
        cookie_security(secure)
    )
}

/// 再認証の短命トークンを消す Cookie。
pub fn clear_elevation_cookie() -> String {
    clear_elevation_cookie_with(true)
}

/// 再認証の短命トークンを消す Cookie。HTTPS 以外のローカル開発では Secure を付けない。
pub fn clear_elevation_cookie_with(secure: bool) -> String {
    format!(
        "{ELEVATION_COOKIE}=; Path=/; HttpOnly{}; SameSite=Lax; Max-Age=0",
        cookie_security(secure)
    )
}

fn cookie_security(secure: bool) -> &'static str {
    if secure {
        "; Secure"
    } else {
        ""
    }
}

/// パスキーの名前を検証し、前後の空白を除いた名前を返す。
pub fn validate_passkey_name(name: &str) -> Result<String, &'static str> {
    let name = name.trim();
    if name.is_empty() {
        return Err("the passkey name is empty");
    }
    if name.chars().count() > 50 {
        return Err("the passkey name is longer than 50 characters");
    }
    Ok(name.to_owned())
}

/// 認証中の利用者と資源の利用者が一致するか。
pub fn is_owner(session_user_id: &str, resource_user_id: &str) -> bool {
    !session_user_id.is_empty() && session_user_id == resource_user_id
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        clear_elevation_cookie_with, clear_oauth_state_cookie, clear_oauth_state_cookie_with,
        clear_session_cookie, clear_session_cookie_with, clear_terminal_cookie,
        clear_terminal_cookie_with, elevation_cookie_with, hash_session_token, hash_terminal_token,
        is_valid_opaque_token, oauth_state_cookie, oauth_state_cookie_with, session_cookie,
        session_cookie_with, sha256_base64url, terminal_cookie, terminal_cookie_with,
        validate_id_token, validate_userinfo, AuthError, OAUTH_STATE_COOKIE, SESSION_COOKIE,
        TERMINAL_COOKIE,
    };

    fn jwt(payload: serde_json::Value) -> String {
        format!(
            "{}.{}.",
            crate::base64url::encode(b"{}"),
            crate::base64url::encode(payload.to_string().as_bytes())
        )
    }

    fn payload() -> serde_json::Value {
        json!({
            "iss": "http://localhost:8788",
            "aud": "warrior-local",
            "sub": "alice",
            "email": "alice@example.test",
            "exp": 1_800_000_000,
            "nonce": "nonce"
        })
    }

    #[test]
    fn session_hash_is_deterministic_and_has_a_fixed_length() {
        assert_eq!(
            sha256_base64url(b"abc"),
            "ungWv48Bz-pBQUDeXa4iI7ADYaOWF3qctBD_YfIAFa0"
        );
        assert_eq!(hash_session_token("token"), hash_session_token("token"));
        assert_eq!(hash_session_token("token").len(), 43);
        assert!(!hash_session_token("token").contains('='));
        assert_ne!(hash_session_token("token"), hash_session_token("other"));
    }

    #[test]
    fn terminal_hash_is_lowercase_hex_and_not_reversible() {
        assert_eq!(
            hash_terminal_token("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(hash_terminal_token("abc").len(), 64);
        assert_eq!(hash_terminal_token("abc"), hash_terminal_token("abc"));
        assert_ne!(hash_terminal_token("abc"), hash_terminal_token("other"));
        assert!(!hash_terminal_token("abc").contains("abc"));
    }

    #[test]
    fn opaque_tokens_are_32_bytes_in_base64url() {
        let token = crate::base64url::encode(&[0u8; 32]);
        assert!(is_valid_opaque_token(&token));
        assert!(!is_valid_opaque_token("short"));
        assert!(!is_valid_opaque_token(&format!("{token}=")));
    }

    #[test]
    fn cookie_attributes_match_the_session_policy() {
        assert_eq!(
            session_cookie("abc"),
            format!(
                "{SESSION_COOKIE}=abc; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=2592000"
            )
        );
        assert_eq!(
            clear_session_cookie(),
            format!("{SESSION_COOKIE}=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0")
        );
        assert_eq!(
            oauth_state_cookie("state"),
            format!(
                "{OAUTH_STATE_COOKIE}=state; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=300"
            )
        );
        assert_eq!(
            clear_oauth_state_cookie(),
            format!("{OAUTH_STATE_COOKIE}=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0")
        );
        assert_eq!(
            terminal_cookie("abc"),
            format!(
                "{TERMINAL_COOKIE}=abc; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=31536000"
            )
        );
        assert_eq!(
            clear_terminal_cookie(),
            format!("{TERMINAL_COOKIE}=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0")
        );
    }

    #[test]
    fn local_http_cookies_omit_the_secure_attribute() {
        for cookie in [
            session_cookie_with("abc", false),
            clear_session_cookie_with(false),
            oauth_state_cookie_with("state", false),
            clear_oauth_state_cookie_with(false),
            terminal_cookie_with("abc", false),
            clear_terminal_cookie_with(false),
            elevation_cookie_with("abc", false),
            clear_elevation_cookie_with(false),
        ] {
            assert!(!cookie.contains("Secure"), "{cookie}");
            assert!(cookie.contains("HttpOnly"), "{cookie}");
            assert!(cookie.contains("SameSite=Lax"), "{cookie}");
        }
    }

    #[test]
    fn id_token_claims_are_validated() {
        let token = jwt(payload());
        let claims = validate_id_token(
            &token,
            "warrior-local",
            "http://localhost:8788",
            "nonce",
            1_700_000_000,
        )
        .unwrap();
        assert_eq!(claims.sub, "alice");
        assert_eq!(claims.email.as_deref(), Some("alice@example.test"));
    }

    #[test]
    fn id_token_rejects_wrong_audience_issuer_nonce_and_expiry() {
        let token = jwt(payload());
        assert_eq!(
            validate_id_token(
                &token,
                "other",
                "http://localhost:8788",
                "nonce",
                1_700_000_000
            ),
            Err(AuthError::InvalidAudience)
        );
        assert_eq!(
            validate_id_token(&token, "warrior-local", "other", "nonce", 1_700_000_000),
            Err(AuthError::InvalidIssuer)
        );
        assert_eq!(
            validate_id_token(
                &token,
                "warrior-local",
                "http://localhost:8788",
                "other",
                1_700_000_000
            ),
            Err(AuthError::InvalidNonce)
        );
        assert_eq!(
            validate_id_token(
                &token,
                "warrior-local",
                "http://localhost:8788",
                "nonce",
                1_800_000_000
            ),
            Err(AuthError::Expired)
        );
    }

    #[test]
    fn id_token_must_have_exactly_three_parts() {
        let token = jwt(payload());
        assert!(validate_id_token(
            &format!("{token}extra.more"),
            "warrior-local",
            "http://localhost:8788",
            "nonce",
            1_700_000_000
        )
        .is_err());
    }

    #[test]
    fn google_accepts_the_issuer_without_a_scheme() {
        let mut value = payload();
        value["iss"] = json!("accounts.google.com");
        let token = jwt(value);
        assert!(validate_id_token(
            &token,
            "warrior-local",
            "https://accounts.google.com",
            "nonce",
            1_700_000_000
        )
        .is_ok());
    }

    #[test]
    fn userinfo_requires_a_matching_verified_user() {
        let userinfo = json!({
            "sub": "alice",
            "email": "alice@example.test",
            "email_verified": true,
            "name": "Alice"
        });
        assert_eq!(validate_userinfo(&userinfo, "alice").unwrap().sub, "alice");
        assert_eq!(
            validate_userinfo(&userinfo, "bob"),
            Err(AuthError::InvalidSubject)
        );
        let mut unverified = userinfo.clone();
        unverified["email_verified"] = json!(false);
        assert_eq!(
            validate_userinfo(&unverified, "alice"),
            Err(AuthError::EmailNotVerified)
        );
    }
}
