//! セッションの解決とログアウト。

use serde::{Deserialize, Serialize};
use warrior_core::auth::{
    clear_session_cookie_with, hash_session_token, is_valid_opaque_token, session_cookie_with,
    SESSION_COOKIE, SESSION_TTL_SECONDS,
};
use warrior_core::sql::{SqlStatement, SqlValue};
use worker::{Env, Request, Response, Result};

use super::{append_cookie, cookie, cookies_secure, now_iso};
use crate::db::Db;
use crate::error::{response, ErrorCode};
use crate::ownership;
use crate::push;

/// 認証済みの利用者。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Session {
    /// 利用者の ID。
    pub id: String,
    /// メールアドレス。
    pub email: String,
    /// 表示名。
    pub display_name: String,
}

#[derive(Debug, Deserialize)]
struct SessionRow {
    user_id: String,
    email: String,
    display_name: String,
}

/// Cookie のセッションを D1 のハッシュと照合して解決する。
pub async fn resolve(request: &Request, env: &Env) -> Result<Option<Session>> {
    let Some(token) = cookie(request, SESSION_COOKIE) else {
        return Ok(None);
    };
    if !is_valid_opaque_token(&token) {
        return Ok(None);
    }
    let db = Db::new(env.d1("DB")?);
    let statement = SqlStatement::new(
        "SELECT s.user_id, u.email, u.display_name \
         FROM auth_sessions s JOIN users u ON u.id = s.user_id \
         WHERE s.token_hash = ? AND s.expires_at > ?",
        vec![
            SqlValue::Text(hash_session_token(&token)),
            SqlValue::Text(now_iso()),
        ],
    );
    let row: Option<SessionRow> = db.first(&statement).await?;
    Ok(row.map(|row| Session {
        id: row.user_id,
        email: row.email,
        display_name: row.display_name,
    }))
}

/// `GET /api/me` の応答。
pub async fn me(request: Request, context: worker::RouteContext<()>) -> Result<Response> {
    let env = context.env;
    match resolve(&request, &env).await? {
        Some(session) => Response::from_json(&session),
        None => Ok(response(ErrorCode::Unauthorized, "authentication required")),
    }
}

/// `POST /api/auth/logout` の応答。
///
/// この端末のプッシュ購読を削除してから、セッションを消す。
pub async fn logout(request: Request, context: worker::RouteContext<()>) -> Result<Response> {
    let env = context.env;
    let db = Db::new(env.d1("DB")?);
    if let Some(authenticated) = resolve(&request, &env).await? {
        if let Some(terminal_id) = ownership::ensure(&db, &request, &authenticated.id).await? {
            let statement = push::delete_for_terminal_statement(&authenticated.id, &terminal_id);
            db.run(&statement).await?;
        }
    }
    if let Some(token) = cookie(&request, SESSION_COOKIE) {
        if is_valid_opaque_token(&token) {
            let statement = SqlStatement::new(
                "DELETE FROM auth_sessions WHERE token_hash = ?",
                vec![SqlValue::Text(hash_session_token(&token))],
            );
            db.run(&statement).await?;
        }
    }
    let body = Response::from_json(&serde_json::json!({ "status": "ok" }))?;
    append_cookie(body, &clear_session_cookie_with(cookies_secure(&env)?))
}

/// セッションを発行する。
pub async fn create(
    db: &Db,
    user_id: &str,
    token: &str,
    now_epoch_seconds: i64,
    secure: bool,
) -> Result<String> {
    let statement = SqlStatement::new(
        "INSERT INTO auth_sessions (id, user_id, token_hash, created_at, expires_at) \
         VALUES (?, ?, ?, ?, ?)",
        vec![
            SqlValue::Text(crate::random::random_uuid()?),
            SqlValue::Text(user_id.to_owned()),
            SqlValue::Text(hash_session_token(token)),
            SqlValue::Text(warrior_core::datetime::format_utc(now_epoch_seconds)),
            SqlValue::Text(warrior_core::datetime::format_utc(
                now_epoch_seconds + SESSION_TTL_SECONDS,
            )),
        ],
    );
    db.run(&statement).await?;
    Ok(session_cookie_with(token, secure))
}
