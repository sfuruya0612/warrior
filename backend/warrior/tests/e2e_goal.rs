//! 種目ごとの目標重量の E2E。
//!
//! 対応する要求: `REQ-GOAL-001`、`REQ-GOAL-002`。
//! 目標を設定し、種目の履歴とグラフに目標重量と現在の記録との差が表示されること、
//! グリップの組を分けて目標を持つこと、目標を取り消せること、AI の出力が無くても
//! 目標の表示が残ることを実ブラウザで確かめる。

mod support;

use std::time::{SystemTime, UNIX_EPOCH};

use support::browser::{self, Browser};
use support::{current_user_id, DevServer, TestOidc};
use thirtyfour::prelude::*;
use warrior_core::jst::{JstDate, UtcInstant};

const T_BAR_ID: &str = "a7c4e2f0-0000-4000-8000-000000000013";
const TRAINING_ID: &str = "a7c4e2f0-0000-4000-8000-000000000301";
const SERVER_PORT: u16 = 8872;
const PROVIDER_PORT: u16 = 8873;
const GOALS: &str = "[data-history-goals]";
const BESTS: &str = "[data-history-bests]";

#[tokio::test]
async fn sets_the_goal_and_shows_the_difference_in_the_history_and_the_graph() {
    let browser = Browser::from_env();
    let origin = format!("http://localhost:{SERVER_PORT}");
    let provider_origin = format!("http://localhost:{PROVIDER_PORT}");
    let server = DevServer::start_with_vars(
        SERVER_PORT,
        &[
            ("ORIGIN", origin.as_str()),
            (
                "GOOGLE_OAUTH_AUTHORIZE_URL",
                &format!("{provider_origin}/authorize"),
            ),
            ("GOOGLE_OAUTH_ISSUER", provider_origin.as_str()),
            (
                "GOOGLE_OAUTH_TOKEN_URL",
                &format!("{provider_origin}/token"),
            ),
            (
                "GOOGLE_OAUTH_USERINFO_URL",
                &format!("{provider_origin}/userinfo"),
            ),
            (
                "GOOGLE_OAUTH_REDIRECT_URI",
                &format!("{origin}/api/auth/google/callback"),
            ),
        ],
    );
    server
        .apply_migrations()
        .expect("the local D1 migrations must apply");
    let provider = TestOidc::start(PROVIDER_PORT);
    let driver_port = browser::free_port();
    let driver_process = browser::DriverProcess::start(browser, driver_port);
    let driver = browser::open(browser, &driver_process).await;

    login(&driver, &server, &provider).await;
    let user_id = current_user_id(&driver).await;
    let today = today();
    let target_date = today.add_days(30).expect("a future day");
    seed_best(&server, &user_id, today);

    open_history(&driver, &server).await;
    browser::wait_for_attribute(&driver, GOALS, "data-history-goals-count", "0").await;
    browser::wait_for_attribute(&driver, BESTS, "data-history-bests-count", "1").await;

    select_filter(&driver, "pronated_medium").await;
    browser::wait_for_present(&driver, "[data-goal-weight]").await;
    set_goal(&driver, 100.0, &target_date).await;
    wait_for_goal(&driver, "100", "90", "10", &target_date).await;

    browser::set_input(&driver, "[data-goal-weight]", "95").await;
    browser::click(&driver, "[data-goal-save]").await;
    wait_for_goal(&driver, "95", "90", "5", &target_date).await;

    select_filter(&driver, "supinated_narrow").await;
    browser::wait_for_attribute(&driver, GOALS, "data-history-goals-count", "0").await;
    select_filter(&driver, "").await;
    browser::wait_for_attribute(&driver, GOALS, "data-history-goals-count", "1").await;

    driver
        .goto(format!("{}/exercises/{T_BAR_ID}/history", server.origin()))
        .await
        .expect("the history must reopen");
    wait_for_goal(&driver, "95", "90", "5", &target_date).await;

    browser::click(&driver, "[data-goal-delete]").await;
    browser::wait_for_attribute(&driver, GOALS, "data-history-goals-count", "0").await;

    browser::assert_no_panic(&driver).await;
    driver.quit().await.expect("the browser must close");
}

async fn login(driver: &WebDriver, server: &DevServer, provider: &TestOidc) {
    driver
        .goto(format!("{}/test/user?sub=goal", provider.origin()))
        .await
        .expect("the test user must be selected");
    browser::login_with_google(driver, server).await;
    browser::wait_for_route(driver, "/").await;
    browser::wait_for_attribute(
        driver,
        "[data-start]",
        "data-active-session-checked",
        "true",
    )
    .await;
}

async fn open_history(driver: &WebDriver, server: &DevServer) {
    driver
        .goto(format!("{}/exercises/{T_BAR_ID}/history", server.origin()))
        .await
        .expect("the exercise history must open");
    browser::wait_for_route(driver, "/exercises/:id/history").await;
}

async fn set_goal(driver: &WebDriver, weight: f64, target_date: &JstDate) {
    browser::set_input(driver, "[data-goal-weight]", &weight.to_string()).await;
    let value = target_date.to_string();
    let mut parts = value.split('-');
    let year = parts.next().expect("the year");
    let month = parts.next().expect("the month");
    let day = parts.next().expect("the day");
    browser::select_value(
        driver,
        "[data-date-input=\"goal-date\"] [data-date-year]",
        year,
    )
    .await;
    browser::select_value(
        driver,
        "[data-date-input=\"goal-date\"] [data-date-month]",
        month,
    )
    .await;
    browser::select_value(
        driver,
        "[data-date-input=\"goal-date\"] [data-date-day]",
        day,
    )
    .await;
    browser::click(driver, "[data-goal-save]").await;
}

async fn wait_for_goal(
    driver: &WebDriver,
    target: &str,
    current: &str,
    difference: &str,
    target_date: &JstDate,
) {
    browser::wait_for_attribute(driver, GOALS, "data-history-goals-count", "1").await;
    let selector = "[data-history-goal]";
    browser::wait_for_attribute(driver, selector, "data-history-goal-target", target).await;
    browser::wait_for_attribute(driver, selector, "data-history-goal-current", current).await;
    browser::wait_for_attribute(driver, selector, "data-history-goal-difference", difference).await;
    browser::wait_for_attribute(
        driver,
        selector,
        "data-history-goal-date",
        &target_date.to_string(),
    )
    .await;
    browser::wait_for_attribute(driver, "[data-graph-target]", "data-graph-target", target).await;
}

async fn select_filter(driver: &WebDriver, value: &str) {
    let select = driver
        .find(By::Css("select[data-history-grip-filter]"))
        .await
        .expect("the grip filter must exist");
    let select = thirtyfour::components::SelectElement::new(&select)
        .await
        .expect("the grip filter must be a select");
    select
        .select_by_value(value)
        .await
        .expect("the filter value must be selectable");
}

fn seed_best(server: &DevServer, user_id: &str, today: JstDate) {
    let date = today.to_string();
    let finished_at = format!("{date}T04:00:00Z");
    for sql in [
        format!(
            "INSERT INTO training_sessions \
             (id, user_id, date, started_at, memo, source_kind, finished_at, total_weight_kg, \
              created_at, updated_at) \
             VALUES ('{TRAINING_ID}', '{user_id}', '{date}', '{date}T02:00:00Z', '', 'empty', \
              '{finished_at}', 0, '{date}T02:00:00Z', '{finished_at}')"
        ),
        format!(
            "INSERT INTO personal_bests \
             (user_id, exercise_id, grip, attachment, kind, value, achieved_on, training_id, \
              created_at, updated_at) \
             VALUES ('{user_id}', '{T_BAR_ID}', 'pronated_medium', '', 'max_weight', 90.0, \
              '{date}', '{TRAINING_ID}', '{date}T04:00:00Z', '{date}T04:00:00Z')"
        ),
    ] {
        server
            .execute_d1_json(&sql)
            .expect("the goal fixture must be seeded");
    }
}

fn today() -> JstDate {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64;
    UtcInstant::from_epoch_seconds(seconds)
        .jst_date()
        .expect("a representable date")
}
