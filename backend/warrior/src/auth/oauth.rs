//! Google OAuth 2.0 の認可コードフロー。
//!
//! state、nonce、PKCE の verifier は D1 に 5 分だけ保存する (ADR-019)。
//! ID トークンの署名は検証せず、TLS で取得した payload の `aud`、`iss`、`exp`、`nonce` を検証する。

use std::cmp::Ordering;
use std::collections::HashMap;

use serde::Deserialize;
use warrior_core::auth::{
    clear_oauth_state_cookie_with, elevation_cookie_with, is_valid_opaque_token,
    oauth_state_cookie_with, sha256_base64url, validate_id_token, validate_userinfo,
    OAUTH_STATE_COOKIE, OAUTH_STATE_TTL_SECONDS,
};
use warrior_core::sql::{SqlStatement, SqlValue};
use worker::{
    console_error, Env, Fetch, Headers, Method, Request, RequestInit, Response, Result,
    RouteContext, Url,
};

use super::session;
use super::{append_cookie, cookie, now_iso, now_seconds, redirect, var};
use crate::db::Db;

const GOOGLE_SCOPE: &str = "openid email profile";

#[derive(Debug, Deserialize)]
struct OAuthStateRow {
    nonce: String,
    code_verifier: String,
    redirect_uri: String,
    expires_at: String,
    used_at: Option<String>,
    purpose: String,
    user_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    id_token: String,
}

#[derive(Debug, Deserialize)]
struct UserIdRow {
    id: String,
}

/// `GET /api/auth/google/start` の処理。
pub async fn start(request: Request, context: RouteContext<()>) -> Result<Response> {
    if let Some(denied) =
        crate::rate_limit::check_ip(&request, &context.env, crate::rate_limit::Kind::AuthIp).await?
    {
        return Ok(denied);
    }
    let secure = super::cookies_secure(&context.env)?;
    begin(&context.env, "login", None, secure).await
}

/// `GET /api/auth/reauthenticate/google/start` の処理。
pub async fn start_elevation(request: Request, context: RouteContext<()>) -> Result<Response> {
    if let Some(denied) =
        crate::rate_limit::check_ip(&request, &context.env, crate::rate_limit::Kind::AuthIp).await?
    {
        return Ok(denied);
    }
    let env = context.env;
    let Some(session) = session::resolve(&request, &env).await? else {
        return Ok(crate::error::response(
            crate::error::ErrorCode::Unauthorized,
            "authentication required",
        ));
    };
    let secure = super::cookies_secure(&env)?;
    begin(&env, "elevation", Some(&session.id), secure).await
}

async fn begin(env: &Env, purpose: &str, user_id: Option<&str>, secure: bool) -> Result<Response> {
    let client_id = var(env, "GOOGLE_CLIENT_ID")?;
    let authorize_url = var(env, "GOOGLE_OAUTH_AUTHORIZE_URL")?;
    let redirect_uri = var(env, "GOOGLE_OAUTH_REDIRECT_URI")?;
    let state = crate::random::random_token()?;
    let nonce = crate::random::random_token()?;
    let code_verifier = crate::random::random_token()?;
    let now = now_seconds();
    let db = Db::new(env.d1("DB")?);
    let cleanup = SqlStatement::new(
        "DELETE FROM oauth_states WHERE expires_at <= ?",
        vec![SqlValue::Text(warrior_core::datetime::format_utc(now))],
    );
    db.run(&cleanup).await?;
    let statement = SqlStatement::new(
        "INSERT INTO oauth_states \
         (state, nonce, code_verifier, redirect_uri, created_at, expires_at, purpose, user_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        vec![
            SqlValue::Text(state.clone()),
            SqlValue::Text(nonce.clone()),
            SqlValue::Text(code_verifier.clone()),
            SqlValue::Text(redirect_uri.clone()),
            SqlValue::Text(warrior_core::datetime::format_utc(now)),
            SqlValue::Text(warrior_core::datetime::format_utc(
                now + OAUTH_STATE_TTL_SECONDS,
            )),
            SqlValue::Text(purpose.to_owned()),
            user_id.map_or(SqlValue::Null, |id| SqlValue::Text(id.to_owned())),
        ],
    );
    db.run(&statement).await?;

    let challenge = sha256_base64url(code_verifier.as_bytes());
    let query = form_body(&[
        ("response_type", "code"),
        ("client_id", &client_id),
        ("redirect_uri", &redirect_uri),
        ("scope", GOOGLE_SCOPE),
        ("state", &state),
        ("nonce", &nonce),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
    ]);
    let separator = if authorize_url.contains('?') {
        '&'
    } else {
        '?'
    };
    let url = Url::parse(&format!("{authorize_url}{separator}{query}"))?;
    let response = redirect(url.as_str())?;
    append_cookie(response, &oauth_state_cookie_with(&state, secure))
}

/// `GET /api/auth/google/callback` の処理。
pub async fn callback(request: Request, context: RouteContext<()>) -> Result<Response> {
    let env = context.env;
    if let Some(denied) =
        crate::rate_limit::check_ip(&request, &env, crate::rate_limit::Kind::AuthIp).await?
    {
        return Ok(denied);
    }
    let query = request
        .url()?
        .query_pairs()
        .into_owned()
        .collect::<HashMap<_, _>>();
    let Some(state) = query.get("state") else {
        return login_error(&env);
    };
    let Some(code) = query.get("code") else {
        return login_error(&env);
    };
    if !is_valid_opaque_token(state)
        || cookie(&request, OAUTH_STATE_COOKIE).as_deref() != Some(state.as_str())
    {
        return login_error(&env);
    }

    let db = Db::new(env.d1("DB")?);
    let statement = SqlStatement::new(
        "SELECT nonce, code_verifier, redirect_uri, expires_at, used_at, purpose, user_id \
         FROM oauth_states WHERE state = ?",
        vec![SqlValue::Text(state.clone())],
    );
    let Some(row) = db.first::<OAuthStateRow>(&statement).await? else {
        return login_error(&env);
    };
    if row.used_at.is_some()
        || warrior_core::datetime::compare_utc(&row.expires_at, &now_iso())
            != Some(Ordering::Greater)
    {
        return login_error(&env);
    }

    let used = SqlStatement::new(
        "UPDATE oauth_states SET used_at = ? WHERE state = ? AND used_at IS NULL",
        vec![SqlValue::Text(now_iso()), SqlValue::Text(state.clone())],
    );
    let result = db.run(&used).await?;
    let changed = result
        .meta()?
        .and_then(|meta| meta.changes.or(meta.rows_written))
        .unwrap_or(0);
    if changed != 1 {
        return login_error(&env);
    }

    let client_id = var(&env, "GOOGLE_CLIENT_ID")?;
    let token_url = var(&env, "GOOGLE_OAUTH_TOKEN_URL")?;
    let client_secret = env
        .secret("GOOGLE_CLIENT_SECRET")
        .map(|secret| secret.to_string())
        .unwrap_or_default();
    let token = match exchange_code(
        &token_url,
        &client_id,
        &client_secret,
        code,
        &row.redirect_uri,
        &row.code_verifier,
    )
    .await
    {
        Ok(token) => token,
        Err(error) => {
            console_error!("the token exchange failed: {error}");
            return login_error(&env);
        }
    };
    let issuer = var(&env, "GOOGLE_OAUTH_ISSUER")?;
    let now = now_seconds();
    let claims = match validate_id_token(&token.id_token, &client_id, &issuer, &row.nonce, now) {
        Ok(claims) => claims,
        Err(error) => {
            console_error!("the ID token is invalid: {error}");
            return login_error(&env);
        }
    };

    let userinfo_url = var(&env, "GOOGLE_OAUTH_USERINFO_URL")?;
    let userinfo = match fetch_userinfo(&userinfo_url, &token.access_token).await {
        Ok(userinfo) => userinfo,
        Err(error) => {
            console_error!("the userinfo request failed: {error}");
            return login_error(&env);
        }
    };
    let userinfo = match validate_userinfo(&userinfo, &claims.sub) {
        Ok(userinfo) => userinfo,
        Err(error) => {
            console_error!("the userinfo is invalid: {error}");
            return login_error(&env);
        }
    };

    let user_id = upsert_user(&db, &userinfo.sub, &userinfo.email, &userinfo.name).await?;
    let origin = var(&env, "ORIGIN")?;
    let secure = super::cookies_secure(&env)?;
    if row.purpose == "elevation" {
        let Some(session) = session::resolve(&request, &env).await? else {
            return Ok(crate::error::response(
                crate::error::ErrorCode::Unauthorized,
                "authentication required",
            ));
        };
        if row.user_id.as_deref() != Some(session.id.as_str()) || session.id != user_id {
            return Ok(crate::error::response(
                crate::error::ErrorCode::Forbidden,
                "reauthentication required",
            ));
        }
        let elevation = super::passkey::issue_elevation(&db, &session.id).await?;
        let url = Url::parse(&format!("{origin}/account"))?;
        let response = redirect(url.as_str())?;
        let response = append_cookie(response, &elevation_cookie_with(&elevation, secure))?;
        return append_cookie(response, &clear_oauth_state_cookie_with(secure));
    }
    let session_token = crate::random::random_token()?;
    let session_cookie = session::create(&db, &user_id, &session_token, now, secure).await?;
    let terminal_cookie = crate::ownership::resolve(&db, &request, &user_id, secure).await?;
    let url = Url::parse(&format!("{origin}/"))?;
    let response = redirect(url.as_str())?;
    let response = append_cookie(response, &session_cookie)?;
    let response = append_cookie(response, &terminal_cookie)?;
    append_cookie(response, &clear_oauth_state_cookie_with(secure))
}

fn login_error(env: &Env) -> Result<Response> {
    let origin = var(env, "ORIGIN")?;
    redirect(&format!("{origin}/login/error"))
}

async fn exchange_code(
    token_url: &str,
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
) -> Result<TokenResponse> {
    let body = form_body(&[
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("code_verifier", code_verifier),
    ]);
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_body(Some(worker::wasm_bindgen::JsValue::from_str(&body)));
    let headers = Headers::new();
    headers.set("Content-Type", "application/x-www-form-urlencoded")?;
    init.with_headers(headers);
    let request = Request::new_with_init(token_url, &init)?;
    let mut response = Fetch::Request(request).send().await?;
    if response.status_code() != 200 {
        console_error!(
            "the token exchange returned the status {}",
            response.status_code()
        );
        return Err(worker::Error::RustError("token exchange failed".into()));
    }
    response.json::<TokenResponse>().await
}

async fn fetch_userinfo(userinfo_url: &str, access_token: &str) -> Result<serde_json::Value> {
    let mut init = RequestInit::new();
    init.with_method(Method::Get);
    let headers = Headers::new();
    headers.set("Authorization", &format!("Bearer {access_token}"))?;
    init.with_headers(headers);
    let request = Request::new_with_init(userinfo_url, &init)?;
    let mut response = Fetch::Request(request).send().await?;
    if response.status_code() != 200 {
        console_error!(
            "the userinfo returned the status {}",
            response.status_code()
        );
        return Err(worker::Error::RustError("userinfo failed".into()));
    }
    response.json::<serde_json::Value>().await
}

async fn upsert_user(db: &Db, google_sub: &str, email: &str, display_name: &str) -> Result<String> {
    let now = now_iso();
    let insert = SqlStatement::new(
        "INSERT INTO users (id, google_sub, email, display_name, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT(google_sub) DO UPDATE SET \
         email = excluded.email, display_name = excluded.display_name, updated_at = excluded.updated_at",
        vec![
            SqlValue::Text(crate::random::random_uuid()?),
            SqlValue::Text(google_sub.to_owned()),
            SqlValue::Text(email.to_owned()),
            SqlValue::Text(display_name.to_owned()),
            SqlValue::Text(now.clone()),
            SqlValue::Text(now),
        ],
    );
    db.run(&insert).await?;
    let select = SqlStatement::new(
        "SELECT id FROM users WHERE google_sub = ?",
        vec![SqlValue::Text(google_sub.to_owned())],
    );
    db.first::<UserIdRow>(&select)
        .await?
        .map(|row| row.id)
        .ok_or_else(|| worker::Error::RustError("the upserted user is missing".into()))
}

fn form_body(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", form_encode(key), form_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::{form_body, form_encode};

    #[test]
    fn form_encoding_escapes_reserved_characters() {
        assert_eq!(form_encode("a b/c:d"), "a%20b%2Fc%3Ad");
        assert_eq!(
            form_body(&[("redirect_uri", "http://localhost:8787/cb")]),
            "redirect_uri=http%3A%2F%2Flocalhost%3A8787%2Fcb"
        );
    }

    #[test]
    fn state_hashes_are_not_reversible() {
        assert_ne!(warrior_core::auth::hash_session_token("token"), "token");
    }
}
