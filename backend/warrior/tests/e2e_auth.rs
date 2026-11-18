//! Google OAuth とセッションの E2E。
//!
//! 対応する要求: `REQ-NFR-006`、`REQ-NFR-013`。
//! Chrome と Safari の実ブラウザで、ログイン、`/api/me`、ログアウト、再ログインを通す。

mod support;

use support::browser::{self, Browser};
use support::{DevServer, TestOidc};
use thirtyfour::{Cookie, SameSite};
use warrior_core::auth::{hash_session_token, hash_terminal_token};

#[tokio::test]
async fn wrangler_google_login_creates_and_revokes_a_session_in_a_browser() {
    let browser = Browser::from_env();
    let server = DevServer::start(8787);
    server
        .apply_migrations()
        .expect("the local D1 migrations must apply");
    let provider = TestOidc::start(8788);
    let driver_port = browser::free_port();
    let driver_process = browser::DriverProcess::start(browser, driver_port);
    let driver = browser::open(browser, &driver_process).await;

    driver
        .goto(format!("{}/test/user?sub=alice", provider.origin()))
        .await
        .expect("the test user must be selected");

    browser::login_with_google(&driver, &server).await;
    let me = browser::get_text(&driver, &format!("{}/api/me", server.origin())).await;
    assert!(me.contains("\"email\":\"alice@example.test\""), "{me}");
    assert!(me.contains("\"display_name\":\"テスト利用者\""), "{me}");
    let first_id = serde_json::from_str::<serde_json::Value>(&me)
        .expect("the me response is JSON")
        .get("id")
        .and_then(|value| value.as_str())
        .expect("the user id")
        .to_owned();

    let cookies = driver
        .get_all_cookies()
        .await
        .expect("the browser cookies must be readable");
    let session = cookies
        .iter()
        .find(|cookie| cookie.name == "warrior_session")
        .expect("the session cookie must exist");
    if browser == Browser::Chrome {
        assert_eq!(
            session.secure,
            Some(false),
            "the local HTTP session cookie must not be Secure"
        );
        assert_eq!(
            session.http_only,
            Some(true),
            "the session cookie must be HttpOnly"
        );
        assert_eq!(session.same_site, Some(SameSite::Lax));
    } else {
        // SafariDriver は Cookie の属性を返さないことがある。属性の正本は結合テストと単体テストで確かめる。
        assert_ne!(session.secure, Some(true));
        assert_ne!(session.http_only, Some(false));
        if let Some(same_site) = session.same_site {
            assert_eq!(same_site, SameSite::Lax);
        }
    }
    let stored = server
        .execute_d1_json("SELECT token_hash FROM auth_sessions")
        .expect("the session rows must be readable");
    let token_hash = stored
        .get(0)
        .and_then(|entry| entry.get("results"))
        .and_then(|rows| rows.get(0))
        .and_then(|row| row.get("token_hash"))
        .and_then(|value| value.as_str())
        .expect("a stored token hash");
    assert_eq!(token_hash, hash_session_token(&session.value));
    assert_ne!(token_hash, session.value);

    let terminal = cookies
        .iter()
        .find(|cookie| cookie.name == "warrior_terminal")
        .expect("the terminal cookie must exist");
    check_terminal_cookie(browser, terminal);
    let stored_terminal = server
        .execute_d1_json("SELECT token_hash FROM terminal_tokens")
        .expect("the terminal rows must be readable");
    let terminal_hash = first_value(&stored_terminal, "token_hash");
    assert_eq!(terminal_hash, hash_terminal_token(&terminal.value));
    assert_ne!(terminal_hash, terminal.value);

    driver
        .goto(format!("{}/", server.origin()))
        .await
        .expect("the home page must open");
    browser::wait_for_route(&driver, "/").await;
    browser::logout(&driver).await;
    let after_logout = browser::get_text(&driver, &format!("{}/api/me", server.origin())).await;
    assert!(
        after_logout.contains("\"code\":\"unauthorized\""),
        "{after_logout}"
    );
    let remaining = server
        .execute_d1_json("SELECT COUNT(*) AS count FROM auth_sessions")
        .expect("the session rows must be readable");
    let count = remaining
        .get(0)
        .and_then(|entry| entry.get("results"))
        .and_then(|rows| rows.get(0))
        .and_then(|row| row.get("count"))
        .and_then(|value| value.as_i64())
        .expect("a session count");
    assert_eq!(count, 0, "logout must delete the session row");
    let terminals = server
        .execute_d1_json("SELECT COUNT(*) AS count FROM terminal_tokens")
        .expect("the terminal rows must be readable");
    assert_eq!(
        first_count(&terminals),
        Some(1),
        "logout must keep the terminal token"
    );

    let error_page = browser::get_text(&driver, &format!("{}/login/error", server.origin())).await;
    assert!(
        error_page.contains("ログインできませんでした"),
        "{error_page}; URL: {:?}",
        driver.current_url().await
    );

    browser::login_with_google(&driver, &server).await;
    let again = browser::get_text(&driver, &format!("{}/api/me", server.origin())).await;
    assert!(
        again.contains("\"email\":\"alice@example.test\""),
        "{again}"
    );
    let second_me =
        serde_json::from_str::<serde_json::Value>(&again).expect("the me response is JSON");
    let second_id = second_me
        .get("id")
        .and_then(|value| value.as_str())
        .expect("the user id");
    assert_eq!(
        first_id, second_id,
        "the same Google sub must keep the user"
    );
    let again_cookies = driver
        .get_all_cookies()
        .await
        .expect("the browser cookies must be readable");
    let again_terminal = again_cookies
        .iter()
        .find(|cookie| cookie.name == "warrior_terminal")
        .expect("the terminal cookie must exist after the login");
    assert_eq!(
        again_terminal.value, terminal.value,
        "the same user must reconnect to the same terminal token"
    );
    let again_terminals = server
        .execute_d1_json("SELECT COUNT(*) AS count FROM terminal_tokens")
        .expect("the terminal rows must be readable");
    assert_eq!(
        first_count(&again_terminals),
        Some(1),
        "the login must reuse the terminal token of the same user"
    );

    driver.quit().await.expect("the browser must close");
}

fn check_terminal_cookie(browser: Browser, terminal: &Cookie) {
    if browser == Browser::Chrome {
        assert_eq!(
            terminal.secure,
            Some(false),
            "the local HTTP terminal cookie must not be Secure"
        );
        assert_eq!(
            terminal.http_only,
            Some(true),
            "the terminal cookie must be HttpOnly"
        );
        assert_eq!(terminal.same_site, Some(SameSite::Lax));
    } else {
        // SafariDriver は Cookie の属性を返さないことがある。属性の正本は結合テストと単体テストで確かめる。
        assert_ne!(terminal.secure, Some(true));
        assert_ne!(terminal.http_only, Some(false));
        if let Some(same_site) = terminal.same_site {
            assert_eq!(same_site, SameSite::Lax);
        }
    }
}

fn first_count(value: &serde_json::Value) -> Option<i64> {
    value.get(0)?.get("results")?.get(0)?.get("count")?.as_i64()
}

fn first_value(value: &serde_json::Value, key: &str) -> String {
    value
        .get(0)
        .and_then(|entry| entry.get("results"))
        .and_then(|rows| rows.get(0))
        .and_then(|row| row.get(key))
        .and_then(|value| value.as_str())
        .expect("a stored value")
        .to_owned()
}
