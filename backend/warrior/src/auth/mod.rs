//! Google OAuth とセッションの HTTP の処理。

pub mod oauth;
pub mod passkey;
pub mod session;

use worker::{Date, Env, Request, Response, Result};

use crate::error::{response, ErrorCode};
use warrior_core::auth;

/// リクエストの Cookie から名前が一致する値を取り出す。
pub fn cookie(request: &Request, name: &str) -> Option<String> {
    let header = request.headers().get("Cookie").ok().flatten()?;
    header.split(';').find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        (key == name).then(|| value.to_owned())
    })
}

/// 環境変数を必須の設定として読む。
pub fn var(env: &Env, name: &str) -> Result<String> {
    Ok(env.var(name)?.to_string())
}

/// Cookie に Secure を付けるか。ORIGIN が HTTPS のときだけ付ける。
pub fn cookies_secure(env: &Env) -> Result<bool> {
    Ok(var(env, "ORIGIN")?.starts_with("https://"))
}

/// 現在の Unix 時刻 (秒)。
pub fn now_seconds() -> i64 {
    (Date::now().as_millis() / 1000) as i64
}

/// 現在の ISO 8601 の時刻。
pub fn now_iso() -> String {
    warrior_core::datetime::format_utc(now_seconds())
}

/// Cookie を応答に追加する。
pub fn append_cookie(mut response: Response, cookie: &str) -> Result<Response> {
    response.headers_mut().append("Set-Cookie", cookie)?;
    Ok(response)
}

/// リダイレクトの応答を作る。
///
/// Workers の `Response::redirect_with_status` はヘッダが変更できないため、
/// Cookie を追加できるように空の応答から組み立てる。
pub fn redirect(location: &str) -> Result<Response> {
    let mut response = Response::empty()?.with_status(302);
    response.headers_mut().set("Location", location)?;
    Ok(response)
}

/// 認証中の利用者と資源の所有者を検査する。一致しなければ 404 にする。
pub fn ensure_owner(session_user_id: &str, resource_user_id: &str) -> Result<(), Response> {
    owner_result(session_user_id, resource_user_id)
        .map_err(|code| response(code, "resource not found"))
}

/// 所有者の検査の結果を状態コードで返す。
pub fn owner_result(session_user_id: &str, resource_user_id: &str) -> Result<(), ErrorCode> {
    if auth::is_owner(session_user_id, resource_user_id) {
        Ok(())
    } else {
        Err(ErrorCode::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::owner_result;
    use crate::error::ErrorCode;

    #[test]
    fn owner_mismatch_is_not_found() {
        assert_eq!(owner_result("user-a", "user-a"), Ok(()));
        assert_eq!(owner_result("user-a", "user-b"), Err(ErrorCode::NotFound));
        assert_eq!(owner_result("", ""), Err(ErrorCode::NotFound));
    }
}
