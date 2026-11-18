//! パスキーの登録、ログイン、再認証、一覧、削除。
//!
//! チャレンジと再認証のトークンは 32 バイトの乱数とし、D1 にはハッシュだけを保存する。
//! 登録、追加、削除はセッションと再認証の短命トークンの両方を要求する (ADR-019)。

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};
use warrior_core::auth::{
    self, elevation_cookie_with, hash_session_token, validate_passkey_name, ELEVATION_COOKIE,
    ELEVATION_TTL_SECONDS, WEBAUTHN_CHALLENGE_TTL_SECONDS,
};
use warrior_core::base64url;
use warrior_core::sql::{SqlStatement, SqlValue};
use warrior_core::webauthn::{self, AuthenticationInput, RegistrationInput, SignCounter};
use worker::{console_error, Env, Request, Response, Result, RouteContext};

use super::session::{self, Session};
use super::{append_cookie, cookie, cookies_secure, now_iso, now_seconds, var};
use crate::db::Db;
use crate::error::{response, ErrorCode};
use crate::rate_limit::{self, Kind};

const KIND_REGISTRATION: &str = "registration";
const KIND_AUTHENTICATION: &str = "authentication";
const KIND_ELEVATION: &str = "elevation";

#[derive(Debug, Deserialize)]
struct ChallengeRow {
    id: String,
    user_id: Option<String>,
    expires_at: String,
}

#[derive(Debug, Deserialize)]
struct ElevationRow {
    user_id: String,
    expires_at: String,
    used_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PasskeyRow {
    id: String,
    user_id: String,
    public_key: String,
    sign_count: i64,
    name: String,
    created_at: String,
    last_used_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoginStartInput {
    email: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RegistrationInputJson {
    name: String,
    credential: RegistrationCredential,
    #[serde(default)]
    transports: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RegistrationCredential {
    response: RegistrationResponse,
}

#[derive(Debug, Deserialize)]
struct RegistrationResponse {
    #[serde(rename = "clientDataJSON")]
    client_data_json: String,
    #[serde(rename = "attestationObject")]
    attestation_object: String,
}

#[derive(Debug, Deserialize)]
struct AuthenticationInputJson {
    credential: AuthenticationCredential,
}

#[derive(Debug, Deserialize)]
struct AuthenticationCredential {
    id: String,
    response: AuthenticationResponse,
}

#[derive(Debug, Deserialize)]
struct AuthenticationResponse {
    #[serde(rename = "clientDataJSON")]
    client_data_json: String,
    #[serde(rename = "authenticatorData")]
    authenticator_data: String,
    signature: String,
}

#[derive(Debug, Serialize)]
struct CreationOptions {
    challenge: String,
    rp: RelyingParty,
    user: UserEntity,
    #[serde(rename = "pubKeyCredParams")]
    pub_key_cred_params: Vec<PublicKeyCredentialParameter>,
    attestation: &'static str,
    #[serde(rename = "authenticatorSelection")]
    authenticator_selection: AuthenticatorSelection,
    timeout: u32,
}

#[derive(Debug, Serialize)]
struct RelyingParty {
    id: String,
    name: &'static str,
}

#[derive(Debug, Serialize)]
struct UserEntity {
    id: String,
    name: String,
    #[serde(rename = "displayName")]
    display_name: String,
}

#[derive(Debug, Serialize)]
struct PublicKeyCredentialParameter {
    #[serde(rename = "type")]
    type_: &'static str,
    alg: i32,
}

#[derive(Debug, Serialize)]
struct AuthenticatorSelection {
    #[serde(rename = "residentKey")]
    resident_key: &'static str,
    #[serde(rename = "userVerification")]
    user_verification: &'static str,
}

#[derive(Debug, Serialize)]
struct RequestOptions {
    challenge: String,
    #[serde(rename = "rpId")]
    rp_id: String,
    #[serde(rename = "userVerification")]
    user_verification: &'static str,
    #[serde(rename = "allowCredentials")]
    allow_credentials: Vec<AllowCredential>,
    timeout: u32,
}

#[derive(Debug, Serialize)]
struct AllowCredential {
    #[serde(rename = "type")]
    type_: &'static str,
    id: String,
    transports: Vec<String>,
}

#[derive(Debug, Serialize)]
struct PasskeyList {
    passkeys: Vec<PasskeyItem>,
}

#[derive(Debug, Serialize)]
struct PasskeyItem {
    id: String,
    name: String,
    created_at: String,
    last_used_at: Option<String>,
}

/// `POST /api/auth/passkey/register/start`
pub async fn register_start(request: Request, context: RouteContext<()>) -> Result<Response> {
    if let Some(denied) = rate_limit::check_ip(&request, &context.env, Kind::PasskeyIp).await? {
        return Ok(denied);
    }
    let env = context.env;
    let Some(session) = session::resolve(&request, &env).await? else {
        return Ok(response(ErrorCode::Unauthorized, "authentication required"));
    };
    let db = Db::new(env.d1("DB")?);
    if !elevation_is_valid(&request, &db, &session.id).await? {
        return Ok(response(ErrorCode::Forbidden, "reauthentication required"));
    }
    let challenge = issue_challenge(&db, Some(&session.id), KIND_REGISTRATION).await?;
    Response::from_json(&creation_options(&challenge, &session, &env)?)
}

/// `POST /api/auth/passkey/register/finish`
pub async fn register_finish(mut request: Request, context: RouteContext<()>) -> Result<Response> {
    if let Some(denied) = rate_limit::check_ip(&request, &context.env, Kind::PasskeyIp).await? {
        return Ok(denied);
    }
    let env = context.env;
    let Some(session) = session::resolve(&request, &env).await? else {
        return Ok(response(ErrorCode::Unauthorized, "authentication required"));
    };
    let db = Db::new(env.d1("DB")?);
    if !elevation_is_valid(&request, &db, &session.id).await? {
        return Ok(response(ErrorCode::Forbidden, "reauthentication required"));
    }
    let input: RegistrationInputJson = match request.json().await {
        Ok(input) => input,
        Err(_) => return Ok(response(ErrorCode::BadRequest, "invalid registration body")),
    };
    let name = match validate_passkey_name(&input.name) {
        Ok(name) => name,
        Err(error) => return Ok(response(ErrorCode::BadRequest, error)),
    };
    let challenge =
        match webauthn::client_data_challenge(&input.credential.response.client_data_json) {
            Ok(challenge) => challenge,
            Err(error) => {
                console_error!("the registration credential is not readable: {error}");
                return Ok(response(ErrorCode::BadRequest, "invalid credential"));
            }
        };
    let Some(row) = find_challenge(&db, &challenge, KIND_REGISTRATION, Some(&session.id)).await?
    else {
        return Ok(response(
            ErrorCode::BadRequest,
            "challenge is missing or used",
        ));
    };
    let rp_id = var(&env, "RP_ID")?;
    let origin = var(&env, "ORIGIN")?;
    let registered = match webauthn::verify_registration(&RegistrationInput {
        rp_id: &rp_id,
        origin: &origin,
        expected_challenge: &challenge,
        client_data_json: &input.credential.response.client_data_json,
        attestation_object: &input.credential.response.attestation_object,
    }) {
        Ok(registered) => registered,
        Err(error) => {
            console_error!("the registration was rejected: {error}");
            return Ok(response(ErrorCode::BadRequest, "invalid credential"));
        }
    };
    if !consume_challenge(&db, &row.id).await? {
        return Ok(response(ErrorCode::Conflict, "challenge is already used"));
    }
    if !consume_elevation(&request, &db, &session.id).await? {
        return Ok(response(ErrorCode::Forbidden, "reauthentication required"));
    }
    let credential_id = base64url::encode(&registered.credential_id);
    if passkey_exists(&db, &credential_id).await? {
        return Ok(response(
            ErrorCode::Conflict,
            "passkey is already registered",
        ));
    }
    let now = now_iso();
    let statement = SqlStatement::new(
        "INSERT INTO passkeys \
         (id, user_id, credential_id, public_key, sign_count, transports, name, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        vec![
            SqlValue::Text(crate::random::random_uuid()?),
            SqlValue::Text(session.id.clone()),
            SqlValue::Text(credential_id),
            SqlValue::Text(base64url::encode(&registered.cose_public_key)),
            SqlValue::Integer(i64::from(registered.sign_count)),
            SqlValue::Text(input.transports.join(",")),
            SqlValue::Text(name),
            SqlValue::Text(now),
        ],
    );
    db.run(&statement).await?;
    list_for_session(&db, &session).await
}

/// `POST /api/auth/passkey/login/start`
pub async fn login_start(mut request: Request, context: RouteContext<()>) -> Result<Response> {
    if let Some(denied) = rate_limit::check_ip(&request, &context.env, Kind::PasskeyIp).await? {
        return Ok(denied);
    }
    let env = context.env;
    let db = Db::new(env.d1("DB")?);
    let input: LoginStartInput = match request.json().await {
        Ok(input) => input,
        Err(_) => LoginStartInput { email: None },
    };
    let user_id = match input
        .email
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
    {
        Some(email) => find_user_by_email(&db, email).await?,
        None => None,
    };
    let allow_credentials = match &user_id {
        Some(user_id) => credentials_for_user(&db, user_id).await?,
        None => Vec::new(),
    };
    let challenge = issue_challenge(&db, user_id.as_deref(), KIND_AUTHENTICATION).await?;
    let rp_id = var(&env, "RP_ID")?;
    Response::from_json(&request_options(&challenge, &rp_id, allow_credentials))
}

/// `POST /api/auth/passkey/login/finish`
pub async fn login_finish(mut request: Request, context: RouteContext<()>) -> Result<Response> {
    if let Some(denied) = rate_limit::check_ip(&request, &context.env, Kind::PasskeyIp).await? {
        return Ok(denied);
    }
    let env = context.env;
    let db = Db::new(env.d1("DB")?);
    let input: AuthenticationInputJson = match request.json().await {
        Ok(input) => input,
        Err(_) => return Ok(response(ErrorCode::BadRequest, "invalid login body")),
    };
    let Some((passkey, user_id, updated_count)) =
        verify_credential(&request, &db, &env, &input, KIND_AUTHENTICATION).await?
    else {
        return Ok(response(ErrorCode::BadRequest, "invalid credential"));
    };
    if !update_counter(&db, &passkey, updated_count).await? {
        return Ok(response(
            ErrorCode::Conflict,
            "signature counter did not increase",
        ));
    }
    let token = crate::random::random_token()?;
    let cookie =
        session::create(&db, &user_id, &token, now_seconds(), cookies_secure(&env)?).await?;
    let terminal_cookie =
        crate::ownership::resolve(&db, &request, &user_id, cookies_secure(&env)?).await?;
    let body = Response::from_json(&serde_json::json!({ "status": "ok" }))?;
    let body = append_cookie(body, &cookie)?;
    append_cookie(body, &terminal_cookie)
}

/// `POST /api/auth/reauthenticate/passkey/start`
pub async fn reauthenticate_start(request: Request, context: RouteContext<()>) -> Result<Response> {
    if let Some(denied) = rate_limit::check_ip(&request, &context.env, Kind::PasskeyIp).await? {
        return Ok(denied);
    }
    let env = context.env;
    let Some(session) = session::resolve(&request, &env).await? else {
        return Ok(response(ErrorCode::Unauthorized, "authentication required"));
    };
    let db = Db::new(env.d1("DB")?);
    let allow_credentials = credentials_for_user(&db, &session.id).await?;
    let challenge = issue_challenge(&db, Some(&session.id), KIND_ELEVATION).await?;
    let rp_id = var(&env, "RP_ID")?;
    Response::from_json(&request_options(&challenge, &rp_id, allow_credentials))
}

/// `POST /api/auth/reauthenticate/passkey/finish`
pub async fn reauthenticate_finish(
    mut request: Request,
    context: RouteContext<()>,
) -> Result<Response> {
    if let Some(denied) = rate_limit::check_ip(&request, &context.env, Kind::PasskeyIp).await? {
        return Ok(denied);
    }
    let env = context.env;
    let Some(session) = session::resolve(&request, &env).await? else {
        return Ok(response(ErrorCode::Unauthorized, "authentication required"));
    };
    let db = Db::new(env.d1("DB")?);
    let input: AuthenticationInputJson = match request.json().await {
        Ok(input) => input,
        Err(_) => return Ok(response(ErrorCode::BadRequest, "invalid login body")),
    };
    let Some((passkey, user_id, updated_count)) =
        verify_credential(&request, &db, &env, &input, KIND_ELEVATION).await?
    else {
        return Ok(response(ErrorCode::BadRequest, "invalid credential"));
    };
    if user_id != session.id {
        return Ok(response(ErrorCode::Forbidden, "reauthentication required"));
    }
    if !update_counter(&db, &passkey, updated_count).await? {
        return Ok(response(
            ErrorCode::Conflict,
            "signature counter did not increase",
        ));
    }
    let token = issue_elevation(&db, &session.id).await?;
    let body = Response::from_json(&serde_json::json!({ "status": "ok" }))?;
    append_cookie(body, &elevation_cookie_with(&token, cookies_secure(&env)?))
}

/// `GET /api/me/passkeys`
pub async fn list(request: Request, context: RouteContext<()>) -> Result<Response> {
    let env = context.env;
    let Some(session) = session::resolve(&request, &env).await? else {
        return Ok(response(ErrorCode::Unauthorized, "authentication required"));
    };
    let db = Db::new(env.d1("DB")?);
    list_for_session(&db, &session).await
}

/// `DELETE /api/me/passkeys/:id`
pub async fn delete(request: Request, context: RouteContext<()>) -> Result<Response> {
    let Some(id) = context.param("id") else {
        return Ok(response(ErrorCode::NotFound, "passkey not found"));
    };
    let id = id.to_owned();
    let env = context.env;
    let Some(session) = session::resolve(&request, &env).await? else {
        return Ok(response(ErrorCode::Unauthorized, "authentication required"));
    };
    let db = Db::new(env.d1("DB")?);
    if !elevation_is_valid(&request, &db, &session.id).await? {
        return Ok(response(ErrorCode::Forbidden, "reauthentication required"));
    }
    let statement = SqlStatement::new(
        "DELETE FROM passkeys WHERE id = ? AND user_id = ?",
        vec![
            SqlValue::Text(id.clone()),
            SqlValue::Text(session.id.clone()),
        ],
    );
    let result = db.run(&statement).await?;
    let changes = result
        .meta()?
        .and_then(|meta| meta.changes.or(meta.rows_written))
        .unwrap_or(0);
    if changes == 0 {
        return Ok(response(ErrorCode::NotFound, "passkey not found"));
    }
    if !consume_elevation(&request, &db, &session.id).await? {
        return Ok(response(ErrorCode::Forbidden, "reauthentication required"));
    }
    Response::from_json(&serde_json::json!({ "id": id }))
}

async fn verify_credential(
    _request: &Request,
    db: &Db,
    env: &Env,
    input: &AuthenticationInputJson,
    kind: &str,
) -> Result<Option<(PasskeyRow, String, Option<u32>)>> {
    let challenge =
        match webauthn::client_data_challenge(&input.credential.response.client_data_json) {
            Ok(challenge) => challenge,
            Err(error) => {
                console_error!("the login credential is not readable: {error}");
                return Ok(None);
            }
        };
    let Some(challenge_row) = find_challenge(db, &challenge, kind, None).await? else {
        console_error!("the challenge is missing for kind {kind}");
        return Ok(None);
    };
    let statement = SqlStatement::new(
        "SELECT id, user_id, public_key, sign_count, name, created_at, last_used_at \
         FROM passkeys WHERE credential_id = ?",
        vec![SqlValue::Text(input.credential.id.clone())],
    );
    let Some(passkey) = db.first::<PasskeyRow>(&statement).await? else {
        console_error!("the passkey is missing for the credential");
        return Ok(None);
    };
    if let Some(expected) = &challenge_row.user_id {
        if expected != &passkey.user_id {
            return Ok(None);
        }
    }
    let public_key = match base64url::decode(&passkey.public_key) {
        Some(public_key) => public_key,
        None => {
            console_error!("the stored public key is not base64url");
            return Ok(None);
        }
    };
    let rp_id = var(env, "RP_ID")?;
    let origin = var(env, "ORIGIN")?;
    let current = match webauthn::verify_authentication(&AuthenticationInput {
        rp_id: &rp_id,
        origin: &origin,
        expected_challenge: &challenge,
        client_data_json: &input.credential.response.client_data_json,
        authenticator_data: &input.credential.response.authenticator_data,
        signature: &input.credential.response.signature,
        cose_public_key: &public_key,
    }) {
        Ok(current) => current,
        Err(error) => {
            console_error!("the login was rejected: {error}");
            return Ok(None);
        }
    };
    let stored = u32::try_from(passkey.sign_count).unwrap_or(0);
    let updated = match webauthn::check_sign_count(stored, current) {
        Ok(SignCounter::Skipped) => None,
        Ok(SignCounter::Updated(value)) => Some(value),
        Err(error) => {
            console_error!("the signature counter was rejected: {error}");
            return Ok(None);
        }
    };
    if !consume_challenge(db, &challenge_row.id).await? {
        return Ok(None);
    }
    let user_id = passkey.user_id.clone();
    Ok(Some((passkey, user_id, updated)))
}

async fn update_counter(db: &Db, passkey: &PasskeyRow, updated: Option<u32>) -> Result<bool> {
    let now = now_iso();
    let statement = match updated {
        Some(updated) => SqlStatement::new(
            "UPDATE passkeys SET sign_count = ?, last_used_at = ? WHERE id = ? AND sign_count < ?",
            vec![
                SqlValue::Integer(i64::from(updated)),
                SqlValue::Text(now),
                SqlValue::Text(passkey.id.clone()),
                SqlValue::Integer(i64::from(updated)),
            ],
        ),
        None => SqlStatement::new(
            "UPDATE passkeys SET last_used_at = ? WHERE id = ?",
            vec![SqlValue::Text(now), SqlValue::Text(passkey.id.clone())],
        ),
    };
    let result = db.run(&statement).await?;
    Ok(result
        .meta()?
        .and_then(|meta| meta.changes.or(meta.rows_written))
        .unwrap_or(0)
        == 1)
}

async fn issue_challenge(db: &Db, user_id: Option<&str>, kind: &str) -> Result<String> {
    let now = now_iso();
    let cleanup = SqlStatement::new(
        "DELETE FROM webauthn_challenges WHERE expires_at <= ?",
        vec![SqlValue::Text(now.clone())],
    );
    db.run(&cleanup).await?;
    let challenge = crate::random::random_token()?;
    let expires =
        warrior_core::datetime::format_utc(now_seconds() + WEBAUTHN_CHALLENGE_TTL_SECONDS);
    let statement = SqlStatement::new(
        "INSERT INTO webauthn_challenges \
         (id, user_id, challenge, kind, created_at, expires_at) VALUES (?, ?, ?, ?, ?, ?)",
        vec![
            SqlValue::Text(crate::random::random_uuid()?),
            user_id.map_or(SqlValue::Null, |id| SqlValue::Text(id.to_owned())),
            SqlValue::Text(challenge.clone()),
            SqlValue::Text(kind.to_owned()),
            SqlValue::Text(now),
            SqlValue::Text(expires),
        ],
    );
    db.run(&statement).await?;
    Ok(challenge)
}

async fn find_challenge(
    db: &Db,
    challenge: &str,
    kind: &str,
    user_id: Option<&str>,
) -> Result<Option<ChallengeRow>> {
    let statement = SqlStatement::new(
        "SELECT id, user_id, expires_at FROM webauthn_challenges \
         WHERE challenge = ? AND kind = ?",
        vec![
            SqlValue::Text(challenge.to_owned()),
            SqlValue::Text(kind.to_owned()),
        ],
    );
    let Some(row) = db.first::<ChallengeRow>(&statement).await? else {
        return Ok(None);
    };
    if user_id.is_some() && row.user_id.as_deref() != user_id {
        return Ok(None);
    }
    if warrior_core::datetime::compare_utc(&row.expires_at, &now_iso()) != Some(Ordering::Greater) {
        return Ok(None);
    }
    Ok(Some(row))
}

async fn consume_challenge(db: &Db, id: &str) -> Result<bool> {
    let statement = SqlStatement::new(
        "DELETE FROM webauthn_challenges WHERE id = ?",
        vec![SqlValue::Text(id.to_owned())],
    );
    let result = db.run(&statement).await?;
    Ok(result
        .meta()?
        .and_then(|meta| meta.changes.or(meta.rows_written))
        .unwrap_or(0)
        == 1)
}

pub async fn issue_elevation(db: &Db, user_id: &str) -> Result<String> {
    let token = crate::random::random_token()?;
    let now = now_seconds();
    let cleanup = SqlStatement::new(
        "DELETE FROM elevation_tokens WHERE expires_at <= ?",
        vec![SqlValue::Text(warrior_core::datetime::format_utc(now))],
    );
    db.run(&cleanup).await?;
    let statement = SqlStatement::new(
        "INSERT INTO elevation_tokens \
         (id, user_id, token_hash, created_at, expires_at) VALUES (?, ?, ?, ?, ?)",
        vec![
            SqlValue::Text(crate::random::random_uuid()?),
            SqlValue::Text(user_id.to_owned()),
            SqlValue::Text(hash_session_token(&token)),
            SqlValue::Text(warrior_core::datetime::format_utc(now)),
            SqlValue::Text(warrior_core::datetime::format_utc(
                now + ELEVATION_TTL_SECONDS,
            )),
        ],
    );
    db.run(&statement).await?;
    Ok(token)
}

async fn elevation_is_valid(request: &Request, db: &Db, user_id: &str) -> Result<bool> {
    let Some(token) = cookie(request, ELEVATION_COOKIE) else {
        return Ok(false);
    };
    if !auth::is_valid_opaque_token(&token) {
        return Ok(false);
    }
    let statement = SqlStatement::new(
        "SELECT user_id, expires_at, used_at FROM elevation_tokens WHERE token_hash = ?",
        vec![SqlValue::Text(hash_session_token(&token))],
    );
    let Some(row) = db.first::<ElevationRow>(&statement).await? else {
        return Ok(false);
    };
    Ok(row.user_id == user_id
        && row.used_at.is_none()
        && warrior_core::datetime::compare_utc(&row.expires_at, &now_iso())
            == Some(Ordering::Greater))
}

async fn consume_elevation(request: &Request, db: &Db, user_id: &str) -> Result<bool> {
    let Some(token) = cookie(request, ELEVATION_COOKIE) else {
        return Ok(false);
    };
    let statement = SqlStatement::new(
        "UPDATE elevation_tokens SET used_at = ? \
         WHERE token_hash = ? AND user_id = ? AND used_at IS NULL AND expires_at > ?",
        vec![
            SqlValue::Text(now_iso()),
            SqlValue::Text(hash_session_token(&token)),
            SqlValue::Text(user_id.to_owned()),
            SqlValue::Text(now_iso()),
        ],
    );
    let result = db.run(&statement).await?;
    Ok(result
        .meta()?
        .and_then(|meta| meta.changes.or(meta.rows_written))
        .unwrap_or(0)
        == 1)
}

async fn passkey_exists(db: &Db, credential_id: &str) -> Result<bool> {
    #[derive(Deserialize)]
    struct CountRow {
        count: i64,
    }
    let statement = SqlStatement::new(
        "SELECT COUNT(*) AS count FROM passkeys WHERE credential_id = ?",
        vec![SqlValue::Text(credential_id.to_owned())],
    );
    Ok(db
        .first::<CountRow>(&statement)
        .await?
        .is_some_and(|row| row.count > 0))
}

async fn find_user_by_email(db: &Db, email: &str) -> Result<Option<String>> {
    let statement = SqlStatement::new(
        "SELECT id FROM users WHERE lower(email) = lower(?)",
        vec![SqlValue::Text(email.to_owned())],
    );
    #[derive(Deserialize)]
    struct UserRow {
        id: String,
    }
    Ok(db.first::<UserRow>(&statement).await?.map(|row| row.id))
}

async fn credentials_for_user(db: &Db, user_id: &str) -> Result<Vec<AllowCredential>> {
    #[derive(Deserialize)]
    struct CredentialRow {
        credential_id: String,
        transports: String,
    }
    let statement = SqlStatement::new(
        "SELECT credential_id, transports FROM passkeys WHERE user_id = ? ORDER BY created_at, id",
        vec![SqlValue::Text(user_id.to_owned())],
    );
    let result = db.all(&statement).await?;
    Ok(result
        .results::<CredentialRow>()?
        .into_iter()
        .map(|row| AllowCredential {
            type_: "public-key",
            id: row.credential_id,
            transports: row
                .transports
                .split(',')
                .filter(|transport| !transport.is_empty())
                .map(str::to_owned)
                .collect(),
        })
        .collect())
}

fn creation_options(challenge: &str, session: &Session, env: &Env) -> Result<CreationOptions> {
    let rp_id = var(env, "RP_ID")?;
    let user_bytes = warrior_core::ids::uuid_bytes(&session.id)
        .ok_or_else(|| worker::Error::RustError("the user id is not a UUID".into()))?;
    Ok(CreationOptions {
        challenge: challenge.to_owned(),
        rp: RelyingParty {
            id: rp_id,
            name: "WARRIOR",
        },
        user: UserEntity {
            id: base64url::encode(&user_bytes),
            name: session.email.clone(),
            display_name: session.display_name.clone(),
        },
        pub_key_cred_params: vec![PublicKeyCredentialParameter {
            type_: "public-key",
            alg: -7,
        }],
        attestation: "none",
        authenticator_selection: AuthenticatorSelection {
            resident_key: "preferred",
            user_verification: "required",
        },
        timeout: timeout_millis(),
    })
}

fn request_options(
    challenge: &str,
    rp_id: &str,
    allow_credentials: Vec<AllowCredential>,
) -> RequestOptions {
    RequestOptions {
        challenge: challenge.to_owned(),
        rp_id: rp_id.to_owned(),
        user_verification: "required",
        allow_credentials,
        timeout: timeout_millis(),
    }
}

fn timeout_millis() -> u32 {
    u32::try_from(WEBAUTHN_CHALLENGE_TTL_SECONDS.saturating_mul(1000)).unwrap_or(u32::MAX)
}

async fn list_for_session(db: &Db, session: &Session) -> Result<Response> {
    let statement = SqlStatement::new(
        "SELECT id, user_id, public_key, sign_count, name, created_at, last_used_at \
         FROM passkeys WHERE user_id = ? ORDER BY created_at, id",
        vec![SqlValue::Text(session.id.clone())],
    );
    let rows = db.all(&statement).await?.results::<PasskeyRow>()?;
    Response::from_json(&PasskeyList {
        passkeys: rows
            .into_iter()
            .map(|row| PasskeyItem {
                id: row.id,
                name: row.name,
                created_at: row.created_at,
                last_used_at: row.last_used_at,
            })
            .collect(),
    })
}
