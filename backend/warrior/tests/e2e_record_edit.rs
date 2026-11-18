//! 過去の記録の編集、削除、日付を指定した追加の E2E。
//!
//! 対応する要求: `REQ-REC-006`。
//! 履歴から過去のトレーニングを開いてセットを編集し、カレンダーに反映されることを確かめる。
//! 続いて履歴から削除し、カレンダーから消えることを確かめる。最後にカレンダーの入口から
//! 日付と所要時間を指定して記録を追加し、その日のトレーニングとして現れることを確かめる。
//! 終了したトレーニングは記録画面を開いた時点でサーバーから端末へ読み込む。

mod support;

use serde_json::json;
use support::browser::{self, Browser};
use support::{current_user_id, seed_profile, DevServer, TestOidc};
use thirtyfour::prelude::*;

const SERVER_PORT: u16 = 8862;
const PROVIDER_PORT: u16 = 8863;
const BENCH_EXERCISE_ID: &str = "a7c4e2f0-0000-4000-8000-000000000003";
const TRAINING_ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c5b01";
const ROW_ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c5b02";
const SET_ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c5b03";
const SEGMENT_ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c5b04";

#[tokio::test]
async fn edits_and_deletes_a_past_record_and_adds_one_with_a_date() {
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
    let driver_process = browser::DriverProcess::start(Browser::Chrome, driver_port);
    let driver = browser::open(Browser::Chrome, &driver_process).await;

    login(&driver, &server, &provider).await;
    let user_id = current_user_id(&driver).await;
    seed_profile(&server, &user_id);
    let date = today();
    seed_finished_training(&server, &user_id, &date);

    // 記録画面を開くと、終了したトレーニングをサーバーから端末へ読み込む。
    driver
        .goto(format!("{origin}/record/{TRAINING_ID}"))
        .await
        .expect("the past record must open");
    browser::wait_for_route(&driver, "/record/:training_id").await;
    browser::wait_for_present(&driver, "[data-record-finished]").await;
    let weight = format!("[data-set-id=\"{SET_ID}\"] [data-set-weight]");
    browser::wait_for_value(&driver, &weight, "60").await;
    browser::set_input(&driver, &weight, "80").await;
    wait_for_segment_weight(&server, 80.0).await;

    // 履歴の一覧に現れ、編集した内容がカレンダーに反映される。
    driver
        .goto(format!("{origin}/history"))
        .await
        .expect("the history must open");
    browser::wait_for_present(&driver, &format!("[data-history-item=\"{TRAINING_ID}\"]")).await;
    driver
        .goto(format!("{origin}/calendar"))
        .await
        .expect("the calendar must open");
    let day = format!("[data-calendar-day=\"{date}\"]");
    browser::wait_for_present(&driver, &day).await;
    browser::click(&driver, &day).await;
    browser::wait_for_present(
        &driver,
        &format!("[data-calendar-training=\"{TRAINING_ID}\"]"),
    )
    .await;
    browser::wait_for_attribute(
        &driver,
        &format!("[data-calendar-training=\"{TRAINING_ID}\"] [data-calendar-set-total]"),
        "data-calendar-set-total",
        "400",
    )
    .await;

    // 履歴から削除すると、一覧とカレンダーから消える。
    driver
        .goto(format!("{origin}/history"))
        .await
        .expect("the history must open");
    let item = format!("[data-history-item=\"{TRAINING_ID}\"]");
    browser::wait_for_present(&driver, &item).await;
    browser::click(&driver, &format!("{item} [data-history-delete]")).await;
    browser::click(&driver, "[data-history-delete-accept]").await;
    browser::wait_for_absent(&driver, &item).await;
    wait_for_training_count(&server, TRAINING_ID, 0).await;

    // カレンダーの入口から日付と所要時間を指定して追加する。
    driver
        .goto(format!("{origin}/calendar"))
        .await
        .expect("the calendar must open");
    browser::wait_for_present(&driver, "[data-calendar-add]").await;
    let mut parts = date.split('-');
    let year = parts.next().expect("the year");
    let month = parts.next().expect("the month");
    let day = parts.next().expect("the day");
    browser::select_value(
        &driver,
        "[data-date-input=\"calendar-add-date\"] [data-date-year]",
        year,
    )
    .await;
    browser::select_value(
        &driver,
        "[data-date-input=\"calendar-add-date\"] [data-date-month]",
        month,
    )
    .await;
    browser::select_value(
        &driver,
        "[data-date-input=\"calendar-add-date\"] [data-date-day]",
        day,
    )
    .await;
    browser::set_input(&driver, "[data-calendar-add-duration]", "30").await;
    browser::click(&driver, "[data-calendar-add-submit]").await;
    browser::wait_for_attribute(
        &driver,
        "[data-calendar-detail]",
        "data-calendar-detail",
        &date,
    )
    .await;
    let added = wait_for_backfilled_training(&server, &date).await;
    assert_ne!(added, TRAINING_ID);
    wait_for_duration(&server, &added, 1_800).await;

    browser::assert_no_panic(&driver).await;
    driver.quit().await.expect("the browser must close");
}

async fn login(driver: &WebDriver, server: &DevServer, provider: &TestOidc) {
    driver
        .goto(format!("{}/test/user?sub=record-edit", provider.origin()))
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

fn today() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock must be after the epoch")
        .as_secs();
    warrior_core::datetime::japan_date(i64::try_from(seconds).expect("the clock fits"))
}

fn seed_finished_training(server: &DevServer, user_id: &str, date: &str) {
    let snapshot = json!({
        "exercise_id": BENCH_EXERCISE_ID,
        "name": "ベンチプレス",
        "body_part": "chest",
        "muscles": [{"muscle": "pectoralis_major_middle", "role": "primary"}],
        "record_mode": "weight_and_reps",
        "laterality": "bilateral",
        "grip_mode": "none",
        "grip_options": []
    });
    server
        .execute_d1_json(&format!(
            "INSERT INTO training_sessions \
             (id, user_id, date, started_at, finished_at, duration_seconds, total_weight_kg, \
              exercise_count, set_count, memo, source_kind, created_at, updated_at) \
             VALUES ('{TRAINING_ID}', '{user_id}', '{date}', '{date}T03:00:00Z', \
              '{date}T04:00:00Z', 3600, 300.0, 1, 1, '', 'empty', \
              '{date}T03:00:00Z', '{date}T04:00:00Z')"
        ))
        .expect("the finished training must be seeded");
    server
        .execute_d1_json(&format!(
            "INSERT INTO training_exercises \
             (id, user_id, training_id, exercise_id, snapshot, position, created_at, updated_at) \
             VALUES ('{ROW_ID}', '{user_id}', '{TRAINING_ID}', '{BENCH_EXERCISE_ID}', \
              '{}', 1, '{date}T03:00:00Z', '{date}T03:00:00Z')",
            snapshot.to_string().replace('\'', "''")
        ))
        .expect("the past exercise must be seeded");
    server
        .execute_d1_json(&format!(
            "INSERT INTO sets \
             (id, user_id, training_exercise_id, position, kind, completed, created_at, updated_at) \
             VALUES ('{SET_ID}', '{user_id}', '{ROW_ID}', 1, 'normal', 1, \
              '{date}T03:00:00Z', '{date}T03:00:00Z')"
        ))
        .expect("the past set must be seeded");
    server
        .execute_d1_json(&format!(
            "INSERT INTO set_segments \
             (id, user_id, set_id, position, weight_kg, reps, momentum_reps, assisted_reps, \
              created_at, updated_at) \
             VALUES ('{SEGMENT_ID}', '{user_id}', '{SET_ID}', 1, 60.0, 5, 0, 0, \
              '{date}T03:00:00Z', '{date}T03:00:00Z')"
        ))
        .expect("the past segment must be seeded");
}

async fn wait_for_segment_weight(server: &DevServer, expected: f64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let rows = server
            .execute_d1_json(&format!(
                "SELECT weight_kg FROM set_segments WHERE id = '{SEGMENT_ID}'"
            ))
            .expect("the segment must be readable");
        let weight = rows
            .get(0)
            .and_then(|entry| entry.get("results"))
            .and_then(|results| results.get(0))
            .and_then(|row| row.get("weight_kg"))
            .and_then(serde_json::Value::as_f64);
        if weight.is_some_and(|weight| (weight - expected).abs() < 1e-9) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the segment weight did not become {expected}: {weight:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

async fn wait_for_training_count(server: &DevServer, training_id: &str, expected: i64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let rows = server
            .execute_d1_json(&format!(
                "SELECT COUNT(*) AS count FROM training_sessions WHERE id = '{training_id}'"
            ))
            .expect("the training must be readable");
        let count = rows
            .get(0)
            .and_then(|entry| entry.get("results"))
            .and_then(|results| results.get(0))
            .and_then(|row| row.get("count"))
            .and_then(serde_json::Value::as_i64);
        if count == Some(expected) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the training count did not become {expected}: {count:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

async fn wait_for_backfilled_training(server: &DevServer, date: &str) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let rows = server
            .execute_d1_json(&format!(
                "SELECT id FROM training_sessions WHERE date = '{date}' \
                 AND started_at IS NULL ORDER BY created_at DESC"
            ))
            .expect("the added training must be readable");
        let id = rows
            .get(0)
            .and_then(|entry| entry.get("results"))
            .and_then(|results| results.get(0))
            .and_then(|row| row.get("id"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        if let Some(id) = id {
            return id;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the record added with a date did not appear"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

async fn wait_for_duration(server: &DevServer, training_id: &str, expected: i64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let rows = server
            .execute_d1_json(&format!(
                "SELECT duration_seconds FROM training_sessions WHERE id = '{training_id}'"
            ))
            .expect("the added training must be readable");
        let duration = rows
            .get(0)
            .and_then(|entry| entry.get("results"))
            .and_then(|results| results.get(0))
            .and_then(|row| row.get("duration_seconds"))
            .and_then(serde_json::Value::as_i64);
        if duration == Some(expected) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the duration did not become {expected}: {duration:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
