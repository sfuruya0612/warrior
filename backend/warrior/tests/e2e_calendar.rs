//! カレンダーの月表示、部位の色、日数の集計、日の詳細、凡例の E2E。
//!
//! 対応する要求: `REQ-CLD-001`、`REQ-CLD-002`、`REQ-CLD-003`、`REQ-CLD-004`、`REQ-CLD-006`。
//! 日付は画面が表示している月を基準に作り、種目とセットは D1 に置く。
//! 月の移動、部位の色と凡例、日数の集計、日の詳細を実ブラウザで確かめる。

mod support;

use std::collections::HashSet;

use serde_json::{json, Value};
use support::browser::{self, Browser};
use support::{current_user_id, DevServer, TestOidc};
use thirtyfour::prelude::*;
use warrior_core::calendar::{next_month, parse_month, previous_month};
use warrior_core::datetime;
use warrior_core::jst::{JstDate, JstMonth};

const SERVER_PORT: u16 = 8812;
const OIDC_PORT: u16 = 8813;
const BENCH: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4901";
const PUSHDOWN: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4902";
const SQUAT: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4903";
const ROW: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4904";
const BENCH_TRAINING_EXERCISE: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4910";
const PUSHDOWN_TRAINING_EXERCISE: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4911";
const SQUAT_TRAINING_EXERCISE: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4912";
const ROW_TRAINING_EXERCISE: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4913";
const WARMUP_SET: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4920";
const NORMAL_SET: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4921";
const PUSHDOWN_SET: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4922";
const SQUAT_SET: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4923";
const ROW_SET: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4924";
const BENCH_GRIP_JSON: &str = r#"{"grip":{"style":"pronated","width":"medium"}}"#;

#[tokio::test]
async fn the_calendar_navigates_and_shows_the_day_detail() {
    let browser = Browser::from_env();
    let server = DevServer::start_with_vars(
        SERVER_PORT,
        &[
            ("ORIGIN", "http://localhost:8812"),
            (
                "GOOGLE_OAUTH_AUTHORIZE_URL",
                "http://localhost:8813/authorize",
            ),
            ("GOOGLE_OAUTH_ISSUER", "http://localhost:8813"),
            ("GOOGLE_OAUTH_TOKEN_URL", "http://localhost:8813/token"),
            (
                "GOOGLE_OAUTH_USERINFO_URL",
                "http://localhost:8813/userinfo",
            ),
            (
                "GOOGLE_OAUTH_REDIRECT_URI",
                "http://localhost:8812/api/auth/google/callback",
            ),
        ],
    );
    server
        .apply_migrations()
        .expect("the local D1 migrations must apply");
    let provider = TestOidc::start(OIDC_PORT);
    let driver_port = browser::free_port();
    let driver_process = browser::DriverProcess::start(browser, driver_port);
    let driver = browser::open(browser, &driver_process).await;
    set_smartphone_viewport(&driver, browser).await;

    login(&driver, &server, &provider).await;
    let user_id = current_user_id(&driver).await;
    driver
        .goto(format!("{}/calendar", server.origin()))
        .await
        .expect("the calendar must open");
    browser::wait_for_route(&driver, "/calendar").await;
    let dates = FixtureDates::displayed(&driver).await;
    seed(&server, &user_id, &dates);
    show_the_month_again(&driver, &dates).await;
    check_the_current_month(&driver, &dates).await;
    check_the_legend(&driver, &dates).await;
    check_the_day_detail(&driver, &dates).await;
    check_the_month_navigation(&driver, &dates).await;

    if browser == Browser::Chrome {
        browser::assert_no_panic(&driver).await;
    }
    driver.quit().await.expect("the browser must close");
}

async fn set_smartphone_viewport(driver: &WebDriver, browser: Browser) {
    if browser != Browser::Chrome {
        return;
    }
    driver
        .cdp()
        .send_raw(
            "Emulation.setDeviceMetricsOverride",
            json!({
                "width": 390,
                "height": 844,
                "deviceScaleFactor": 1.0,
                "mobile": true
            }),
        )
        .await
        .expect("the smartphone viewport must be set");
}

async fn login(driver: &WebDriver, server: &DevServer, provider: &TestOidc) {
    driver
        .goto(format!("{}/test/user?sub=calendar", provider.origin()))
        .await
        .expect("the test user must be selected");
    browser::login_with_google(driver, server).await;
    browser::wait_for_attribute(
        driver,
        "[data-start]",
        "data-active-session-checked",
        "true",
    )
    .await;
}

async fn check_the_current_month(driver: &WebDriver, dates: &FixtureDates) {
    browser::wait_for_attribute(
        driver,
        "[data-calendar]",
        "data-calendar-month",
        &dates.current_month,
    )
    .await;
    browser::wait_for_attribute(
        driver,
        "[data-calendar-training-days]",
        "data-calendar-training-days",
        "2",
    )
    .await;
    assert_eq!(
        browser::attribute(
            driver,
            "[data-calendar-part=\"chest\"]",
            "data-calendar-part-days"
        )
        .await,
        "1"
    );
    assert_eq!(
        browser::attribute(
            driver,
            "[data-calendar-part=\"triceps\"]",
            "data-calendar-part-days"
        )
        .await,
        "1"
    );
    assert_eq!(
        browser::attribute(
            driver,
            "[data-calendar-part=\"legs\"]",
            "data-calendar-part-days"
        )
        .await,
        "1"
    );
    assert_eq!(
        browser::attribute(
            driver,
            "[data-calendar-part=\"back\"]",
            "data-calendar-part-days"
        )
        .await,
        "0"
    );
    assert_eq!(
        browser::attribute(driver, &day_selector(dates.first), "data-calendar-parts").await,
        "chest,triceps"
    );
    assert_eq!(
        browser::attribute(driver, &day_selector(dates.second), "data-calendar-parts").await,
        "legs"
    );
    assert_eq!(
        browser::attribute(driver, &day_selector(dates.rest), "data-calendar-parts").await,
        ""
    );
    let cells = eval_json(
        driver,
        "const done = arguments[arguments.length - 1]; \
         done(JSON.stringify(document.querySelectorAll('[data-calendar-day]').length));",
    )
    .await;
    assert_eq!(cells.as_f64(), Some(f64::from(dates.days_in_month)));
    browser::wait_for_text(driver, "カレンダー").await;
}

async fn check_the_legend(driver: &WebDriver, dates: &FixtureDates) {
    let colors = eval_json(
        driver,
        "const done = arguments[arguments.length - 1]; \
         const colors = Array.from(document.querySelectorAll('[data-calendar-swatch]')) \
           .map((element) => getComputedStyle(element).backgroundColor); \
         done(JSON.stringify(colors));",
    )
    .await;
    let colors = colors
        .as_array()
        .expect("the swatch colors are an array")
        .iter()
        .map(|value| value.as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(colors.len(), 8, "the legend lists the eight parts");
    let unique = colors.iter().collect::<HashSet<_>>();
    assert_eq!(unique.len(), 8, "each part has its own color");
    let markers = eval_json(
        driver,
        &format!(
            "const done = arguments[arguments.length - 1]; \
             const markers = Array.from(document.querySelectorAll('{} [data-calendar-marker]')) \
               .map((element) => ({{ \
                 part: element.getAttribute('data-calendar-marker'), \
                 color: getComputedStyle(element).backgroundColor \
               }})); \
             done(JSON.stringify(markers));",
            day_selector(dates.first)
        ),
    )
    .await;
    let markers = markers.as_array().expect("the markers are an array");
    assert_eq!(
        markers
            .iter()
            .map(|marker| marker["part"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        vec!["chest", "triceps"],
        "the colors of a multi-part day are shown side by side"
    );
    assert_eq!(
        markers[0]["color"].as_str().map(str::to_owned),
        Some(legend_color(driver, "chest").await)
    );
    assert_eq!(
        markers[1]["color"].as_str().map(str::to_owned),
        Some(legend_color(driver, "triceps").await)
    );
    assert_ne!(markers[0]["color"], markers[1]["color"]);
}

async fn check_the_day_detail(driver: &WebDriver, dates: &FixtureDates) {
    driver
        .find(By::Css(day_selector(dates.first)))
        .await
        .expect("the first day must exist")
        .click()
        .await
        .expect("the first day must be clickable");
    browser::wait_for_attribute(
        driver,
        "[data-calendar-detail]",
        "data-calendar-detail",
        &dates.first.to_string(),
    )
    .await;
    browser::wait_for_attribute(
        driver,
        "[data-calendar-day-total]",
        "data-calendar-day-total",
        "860",
    )
    .await;
    let exercises = eval_json(
        driver,
        "const done = arguments[arguments.length - 1]; \
         const items = Array.from(document.querySelectorAll('[data-calendar-exercise]')).map((element) => ({ \
           name: element.getAttribute('data-calendar-exercise-name'), \
           total: Number(element.getAttribute('data-calendar-exercise-total')), \
           sets: Array.from(element.querySelectorAll('[data-calendar-set]')).map((set) => ({ \
             kind: set.getAttribute('data-calendar-set-kind'), \
             text: set.getAttribute('data-calendar-set-text'), \
             notes: set.getAttribute('data-calendar-set-notes'), \
             total: Number(set.getAttribute('data-calendar-set-total')) \
           })) \
         })); \
         done(JSON.stringify(items));",
    )
    .await;
    let exercises = exercises.as_array().expect("the exercises are an array");
    assert_eq!(exercises.len(), 2);
    assert_eq!(exercises[0]["name"], "ベンチプレス");
    assert_eq!(exercises[0]["total"].as_f64(), Some(500.0));
    assert_eq!(exercises[0]["sets"].as_array().map(Vec::len), Some(2));
    assert_eq!(exercises[0]["sets"][0]["kind"], "warmup");
    assert_eq!(exercises[0]["sets"][0]["text"], "40 kg × 10 回");
    assert_eq!(exercises[0]["sets"][0]["notes"], "ウォームアップ");
    assert_eq!(exercises[0]["sets"][0]["total"].as_f64(), Some(0.0));
    assert_eq!(exercises[0]["sets"][1]["kind"], "normal");
    assert_eq!(exercises[0]["sets"][1]["text"], "100 kg × 5 回");
    assert_eq!(
        exercises[0]["sets"][1]["notes"],
        "通常 / RPE 8.0 / 順手 ミディアム"
    );
    assert_eq!(exercises[0]["sets"][1]["total"].as_f64(), Some(500.0));
    assert_eq!(exercises[1]["name"], "プレスダウン");
    assert_eq!(exercises[1]["sets"][0]["text"], "30 kg × 12 回");
    assert_eq!(exercises[1]["total"].as_f64(), Some(360.0));
    browser::wait_for_text(driver, "総重量").await;
}

async fn check_the_month_navigation(driver: &WebDriver, dates: &FixtureDates) {
    click(driver, "[data-calendar-next]").await;
    browser::wait_for_attribute(
        driver,
        "[data-calendar]",
        "data-calendar-month",
        &dates.next_month,
    )
    .await;
    browser::wait_for_attribute(
        driver,
        "[data-calendar-training-days]",
        "data-calendar-training-days",
        "0",
    )
    .await;
    browser::wait_for_attribute(
        driver,
        &day_selector(dates.next_first),
        "data-calendar-future",
        "true",
    )
    .await;
    assert_eq!(
        day_number_color(driver, dates.next_first).await,
        subtle_text_color(driver).await,
        "a day after today uses the subtle text color"
    );

    click(driver, "[data-calendar-previous]").await;
    browser::wait_for_attribute(
        driver,
        "[data-calendar]",
        "data-calendar-month",
        &dates.current_month,
    )
    .await;
    browser::wait_for_attribute(
        driver,
        "[data-calendar-training-days]",
        "data-calendar-training-days",
        "2",
    )
    .await;

    click(driver, "[data-calendar-previous]").await;
    browser::wait_for_attribute(
        driver,
        "[data-calendar]",
        "data-calendar-month",
        &dates.previous_month,
    )
    .await;
    browser::wait_for_attribute(
        driver,
        "[data-calendar-training-days]",
        "data-calendar-training-days",
        "1",
    )
    .await;
    assert_eq!(
        browser::attribute(
            driver,
            &day_selector(dates.previous_day),
            "data-calendar-parts"
        )
        .await,
        "back"
    );
    assert_eq!(
        browser::attribute(
            driver,
            "[data-calendar-part=\"back\"]",
            "data-calendar-part-days"
        )
        .await,
        "1"
    );

    click(driver, "[data-calendar-next]").await;
    browser::wait_for_attribute(
        driver,
        "[data-calendar]",
        "data-calendar-month",
        &dates.current_month,
    )
    .await;
}

async fn click(driver: &WebDriver, selector: &str) {
    driver
        .find(By::Css(selector))
        .await
        .unwrap_or_else(|error| panic!("the button {selector} must exist: {error}"))
        .click()
        .await
        .unwrap_or_else(|error| panic!("the button {selector} must be clickable: {error}"));
}

async fn legend_color(driver: &WebDriver, part: &str) -> String {
    eval_json(
        driver,
        &format!(
            "const done = arguments[arguments.length - 1]; \
             const swatch = document.querySelector('[data-calendar-swatch=\"{part}\"]'); \
             done(JSON.stringify(swatch ? getComputedStyle(swatch).backgroundColor : ''));"
        ),
    )
    .await
    .as_str()
    .unwrap_or_default()
    .to_owned()
}

async fn day_number_color(driver: &WebDriver, date: JstDate) -> String {
    eval_json(
        driver,
        &format!(
            "const done = arguments[arguments.length - 1]; \
             const number = document.querySelector('{} > span'); \
             done(JSON.stringify(number ? getComputedStyle(number).color : ''));",
            day_selector(date)
        ),
    )
    .await
    .as_str()
    .unwrap_or_default()
    .to_owned()
}

async fn subtle_text_color(driver: &WebDriver) -> String {
    eval_json(
        driver,
        "const done = arguments[arguments.length - 1]; \
         const label = document.querySelector('[data-calendar-training-days]').parentElement; \
         done(JSON.stringify(label ? getComputedStyle(label).color : ''));",
    )
    .await
    .as_str()
    .unwrap_or_default()
    .to_owned()
}

fn day_selector(date: JstDate) -> String {
    format!("[data-calendar-day=\"{date}\"]")
}

async fn eval_json(driver: &WebDriver, script: &str) -> Value {
    browser::eval_json(driver, script, Vec::new()).await
}

/// E2E で作る日付。画面が表示している月と、その前後の月を使う。
struct FixtureDates {
    first: JstDate,
    second: JstDate,
    rest: JstDate,
    previous_day: JstDate,
    next_first: JstDate,
    days_in_month: u32,
    current_month: String,
    next_month: String,
    previous_month: String,
}

impl FixtureDates {
    /// 画面が表示している月を基準にする。端末の時計が月をまたいでも、画面の月と種の月が食い違わない。
    async fn displayed(driver: &WebDriver) -> Self {
        driver
            .find(By::Css("[data-calendar]"))
            .await
            .expect("the calendar must render");
        let text = browser::attribute(driver, "[data-calendar]", "data-calendar-month").await;
        let month = parse_month(&text).expect("the displayed month must be YYYY-MM");
        Self::for_month(month)
    }

    fn for_month(month: JstMonth) -> Self {
        let first = month.start();
        let previous_day = first.add_days(-1).expect("a previous day");
        Self {
            first,
            second: first.add_days(1).expect("a second day"),
            rest: first.add_days(2).expect("a rest day"),
            previous_day,
            next_first: month
                .end()
                .add_days(1)
                .expect("a first day of the next month"),
            days_in_month: u32::from(month.end().day()),
            current_month: month_text(first),
            next_month: month_text(next_month(month).expect("a next month").start()),
            previous_month: month_text(previous_month(month).expect("a previous month").start()),
        }
    }
}

/// 種を置いた後に、表示中の月を取り直す。次の月へ移ってから戻ると、表示中の月を読み直す。
async fn show_the_month_again(driver: &WebDriver, dates: &FixtureDates) {
    click(driver, "[data-calendar-next]").await;
    browser::wait_for_attribute(
        driver,
        "[data-calendar]",
        "data-calendar-month",
        &dates.next_month,
    )
    .await;
    click(driver, "[data-calendar-previous]").await;
    browser::wait_for_attribute(
        driver,
        "[data-calendar]",
        "data-calendar-month",
        &dates.current_month,
    )
    .await;
}

fn month_text(date: JstDate) -> String {
    format!("{:04}-{:02}", date.year(), date.month())
}

fn seed(server: &DevServer, user_id: &str, dates: &FixtureDates) {
    let mut statements = Vec::new();
    for (index, (date, name)) in [
        (dates.first, "current-first"),
        (dates.second, "current-second"),
        (dates.previous_day, "previous-month"),
    ]
    .into_iter()
    .enumerate()
    {
        statements.push(session_sql(
            &format!("3f2504e0-4f89-41d3-9a0c-0305e82c493{index}"),
            user_id,
            date,
            name,
        ));
    }
    statements.extend([
        exercise_sql(
            user_id,
            &training_id(0),
            BENCH_TRAINING_EXERCISE,
            BENCH,
            "ベンチプレス",
            "chest",
            1,
        ),
        exercise_sql(
            user_id,
            &training_id(0),
            PUSHDOWN_TRAINING_EXERCISE,
            PUSHDOWN,
            "プレスダウン",
            "triceps",
            2,
        ),
        exercise_sql(
            user_id,
            &training_id(1),
            SQUAT_TRAINING_EXERCISE,
            SQUAT,
            "スクワット",
            "legs",
            1,
        ),
        exercise_sql(
            user_id,
            &training_id(2),
            ROW_TRAINING_EXERCISE,
            ROW,
            "シーテッドロウ",
            "back",
            1,
        ),
        set_sql(
            user_id,
            BENCH_TRAINING_EXERCISE,
            WARMUP_SET,
            1,
            "warmup",
            None,
        ),
        set_sql(
            user_id,
            BENCH_TRAINING_EXERCISE,
            NORMAL_SET,
            2,
            "normal",
            Some(BENCH_GRIP_JSON),
        ),
        set_sql(
            user_id,
            PUSHDOWN_TRAINING_EXERCISE,
            PUSHDOWN_SET,
            1,
            "normal",
            None,
        ),
        set_sql(
            user_id,
            SQUAT_TRAINING_EXERCISE,
            SQUAT_SET,
            1,
            "normal",
            None,
        ),
        set_sql(user_id, ROW_TRAINING_EXERCISE, ROW_SET, 1, "normal", None),
        segment_sql(
            user_id,
            WARMUP_SET,
            "3f2504e0-4f89-41d3-9a0c-0305e82c4940",
            1,
            40.0,
            10,
        ),
        segment_sql(
            user_id,
            NORMAL_SET,
            "3f2504e0-4f89-41d3-9a0c-0305e82c4941",
            1,
            100.0,
            5,
        ),
        segment_sql(
            user_id,
            PUSHDOWN_SET,
            "3f2504e0-4f89-41d3-9a0c-0305e82c4942",
            1,
            30.0,
            12,
        ),
        segment_sql(
            user_id,
            SQUAT_SET,
            "3f2504e0-4f89-41d3-9a0c-0305e82c4943",
            1,
            80.0,
            8,
        ),
        segment_sql(
            user_id,
            ROW_SET,
            "3f2504e0-4f89-41d3-9a0c-0305e82c4944",
            1,
            50.0,
            10,
        ),
    ]);
    for sql in statements {
        server
            .execute_d1_json(&sql)
            .expect("the calendar fixture must be seeded");
    }
    server
        .execute_d1_json(&format!(
            "UPDATE sets SET rpe = 8.0 WHERE id = '{NORMAL_SET}'"
        ))
        .expect("the RPE must be stored");
}

fn training_id(index: usize) -> String {
    format!("3f2504e0-4f89-41d3-9a0c-0305e82c493{index}")
}

fn session_sql(training_id: &str, user_id: &str, date: JstDate, memo: &str) -> String {
    let started_at = datetime::format_utc(date.start_utc().seconds() + 3 * 3_600);
    format!(
        "INSERT INTO training_sessions (id, user_id, date, started_at, memo, created_at, updated_at) \
         VALUES ('{training_id}', '{user_id}', '{date}', '{started_at}', '{memo}', \
         '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"
    )
}

fn exercise_sql(
    user_id: &str,
    training_id: &str,
    training_exercise_id: &str,
    exercise_id: &str,
    name: &str,
    body_part: &str,
    position: i64,
) -> String {
    let snapshot = json!({
        "exercise_id": exercise_id,
        "name": name,
        "body_part": body_part,
        "muscles": [],
        "record_mode": "weight_and_reps",
        "laterality": "bilateral",
        "grip_mode": "none",
        "grip_options": []
    })
    .to_string()
    .replace('\'', "''");
    format!(
        "INSERT INTO training_exercises (id, user_id, training_id, exercise_id, snapshot, position, \
         created_at, updated_at) \
         VALUES ('{training_exercise_id}', '{user_id}', '{training_id}', '{exercise_id}', \
         '{snapshot}', {position}, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"
    )
}

fn set_sql(
    user_id: &str,
    training_exercise_id: &str,
    set_id: &str,
    position: i64,
    kind: &str,
    grip: Option<&str>,
) -> String {
    let grip = grip.map_or("NULL".to_owned(), |value| format!("'{value}'"));
    format!(
        "INSERT INTO sets (id, user_id, training_exercise_id, position, kind, completed, grip, \
         created_at, updated_at) \
         VALUES ('{set_id}', '{user_id}', '{training_exercise_id}', {position}, '{kind}', 1, \
         {grip}, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"
    )
}

fn segment_sql(
    user_id: &str,
    set_id: &str,
    segment_id: &str,
    position: i64,
    weight_kg: f64,
    reps: i64,
) -> String {
    format!(
        "INSERT INTO set_segments (id, user_id, set_id, position, weight_kg, reps, momentum_reps, \
         assisted_reps, duration_seconds, created_at, updated_at) \
         VALUES ('{segment_id}', '{user_id}', '{set_id}', {position}, {weight_kg}, {reps}, 0, 0, \
         NULL, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"
    )
}
