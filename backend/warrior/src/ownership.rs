//! 端末トークンの発行と照合 (ADR-003)。
//!
//! 端末トークンは 32 バイトの乱数とし、HttpOnly、SameSite=Lax の Cookie で渡す。
//! HTTPS では Secure を付け、ローカルの HTTP では付けない。
//! D1 には SHA-256 のハッシュだけを保存し、応答とログには値を出さない。
//! 端末トークンは利用者に属し、認証中の利用者と一致する行だけを使う。

use serde::Deserialize;
use warrior_core::auth::{
    hash_terminal_token, is_valid_opaque_token, terminal_cookie_with, TERMINAL_COOKIE,
};
use warrior_core::sql::{SqlStatement, SqlValue};
use worker::{Request, Result};

use crate::auth::{cookie, now_iso};
use crate::db::Db;

#[derive(Debug, Deserialize)]
struct TerminalRow {
    id: String,
    user_id: String,
}

/// ログインした利用者の端末トークンを解決し、設定する Cookie を返す。
///
/// 既存の Cookie の行が同じ利用者のものであれば再利用し、最終使用の時刻を更新する。
/// Cookie が無い場合と、別の利用者の行であった場合は、新しいトークンを作る。
/// 別の利用者の行は同じ端末を別のアカウントで使ったものなので削除する。
pub async fn resolve(db: &Db, request: &Request, user_id: &str, secure: bool) -> Result<String> {
    if let Some(existing) = existing_token(request) {
        let hash = hash_terminal_token(&existing);
        if let Some(row) = find(db, &hash).await? {
            if row.user_id == user_id {
                touch(db, &hash).await?;
                return Ok(terminal_cookie_with(&existing, secure));
            }
            delete(db, &hash).await?;
        }
    }
    let token = crate::random::random_token()?;
    let now = now_iso();
    let statement = SqlStatement::new(
        "INSERT INTO terminal_tokens (id, user_id, token_hash, created_at, last_seen_at) \
         VALUES (?, ?, ?, ?, ?)",
        vec![
            SqlValue::Text(crate::random::random_uuid()?),
            SqlValue::Text(user_id.to_owned()),
            SqlValue::Text(hash_terminal_token(&token)),
            SqlValue::Text(now.clone()),
            SqlValue::Text(now),
        ],
    );
    db.run(&statement).await?;
    Ok(terminal_cookie_with(&token, secure))
}

/// 認証中の要求の端末トークンの行の ID を返す。
///
/// Cookie が無い、形が不正、または別の利用者の行である場合は None を返す。
pub async fn ensure(db: &Db, request: &Request, user_id: &str) -> Result<Option<String>> {
    let Some(token) = existing_token(request) else {
        return Ok(None);
    };
    let hash = hash_terminal_token(&token);
    let Some(row) = find(db, &hash).await? else {
        return Ok(None);
    };
    if row.user_id != user_id {
        return Ok(None);
    }
    touch(db, &hash).await?;
    Ok(Some(row.id))
}

fn existing_token(request: &Request) -> Option<String> {
    cookie(request, TERMINAL_COOKIE).filter(|token| is_valid_opaque_token(token))
}

async fn find(db: &Db, hash: &str) -> Result<Option<TerminalRow>> {
    db.first(&SqlStatement::new(
        "SELECT id, user_id FROM terminal_tokens WHERE token_hash = ?",
        vec![SqlValue::Text(hash.to_owned())],
    ))
    .await
}

async fn touch(db: &Db, hash: &str) -> Result<()> {
    let statement = SqlStatement::new(
        "UPDATE terminal_tokens SET last_seen_at = ? WHERE token_hash = ?",
        vec![SqlValue::Text(now_iso()), SqlValue::Text(hash.to_owned())],
    );
    db.run(&statement).await?;
    Ok(())
}

async fn delete(db: &Db, hash: &str) -> Result<()> {
    let statement = SqlStatement::new(
        "DELETE FROM terminal_tokens WHERE token_hash = ?",
        vec![SqlValue::Text(hash.to_owned())],
    );
    db.run(&statement).await?;
    Ok(())
}
