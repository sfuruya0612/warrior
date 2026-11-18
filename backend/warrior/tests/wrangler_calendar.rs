//! カレンダーの月の集計と日の詳細の結合テスト。
//!
//! 対応する要求: `REQ-CLD-001`、`REQ-CLD-002`、`REQ-CLD-003`、`REQ-CLD-004`、`REQ-CLD-006`。
//! 月の境界を日本時間の日付で分け、日ごとの部位を記録時のスナップショットから判定し、
//! 月のトレーニング日数と部位区分ごとの日数を集計し、日の詳細が種目、セット、総重量を返すことを確かめる。
//! 種目とセットは D1 に置き、記録画面は使わない。

mod support;

use serde_json::{json, Value};
use support::DevServer;
use warrior_core::auth::hash_session_token;
use warrior_core::base64url;
use warrior_core::calendar::{CalendarDayDetail, CalendarMonth};
use warrior_core::exercise::BodyPart;
use warrior_core::jst::JstDate;
use warrior_core::set_calc::{RecordMode, SetKind};

const PORT: u16 = 8811;
const USER_ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4801";
const OTHER_USER_ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4802";
const JAN_FIRST: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4810";
const JAN_SECOND: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4811";
const JAN_SECOND_EMPTY: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4815";
const JAN_SECOND_WITHOUT_START: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4816";
const JAN_LAST: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4812";
const FEB_FIRST: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4813";
const OTHER_JAN: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4814";
const BENCH: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4820";
const PUSHDOWN: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4821";
const ROW: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4822";
const SQUAT: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4823";
const CRUNCH: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4824";
const BENCH_TRAINING_EXERCISE: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4830";
const PUSHDOWN_TRAINING_EXERCISE: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4831";
const ROW_TRAINING_EXERCISE: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4832";
const SQUAT_TRAINING_EXERCISE: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4833";
const FEB_SQUAT_TRAINING_EXERCISE: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4834";
const OTHER_TRAINING_EXERCISE: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4835";
const WARMUP_SET: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4840";
const NORMAL_SET: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4841";
const DROP_SET: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4842";
const PUSHDOWN_SET: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4843";
const ROW_SET: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4844";
const SQUAT_SET: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4845";
const FEB_SQUAT_SET: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4846";
const CRUNCH_SET: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c4847";
const BENCH_GRIP_JSON: &str = r#"{"grip":{"style":"pronated","width":"medium"}}"#;

#[test]
fn the_calendar_returns_the_month_and_the_day_details_for_its_owner() {
    let server = DevServer::start(PORT);
    server
        .apply_migrations()
        .expect("the local D1 migrations must apply");
    let token = base64url::encode(&[21u8; 32]);
    seed(&server, &token);
    let cookie = format!("warrior_session={token}");

    check_the_january_month(&server, &cookie);
    check_the_snapshot_body_part(&server, &cookie);
    check_the_month_boundary(&server, &cookie);
    check_the_empty_month(&server, &cookie);
    check_the_day_detail(&server, &cookie);
    check_the_day_orders_sessions_without_a_start(&server, &cookie);
    check_the_day_without_training(&server, &cookie);
    check_rejections(&server, &cookie);
}

fn check_the_january_month(server: &DevServer, cookie: &str) {
    let view = month(server, cookie, "2026-01");
    assert_eq!(view.month, "2026-01");
    assert_eq!(view.start, JstDate::new(2026, 1, 1).unwrap());
    assert_eq!(view.end, JstDate::new(2026, 1, 31).unwrap());
    assert_eq!(
        view.days
            .iter()
            .map(|day| (day.date.day(), day.body_parts.clone()))
            .collect::<Vec<_>>(),
        vec![
            (10, vec![BodyPart::Chest, BodyPart::Triceps]),
            (20, vec![BodyPart::Back]),
            (31, vec![BodyPart::Legs]),
        ]
    );
    assert_eq!(view.training_days, 3);
    assert_eq!(
        counts(&view),
        vec![
            (BodyPart::Chest, 1),
            (BodyPart::Back, 1),
            (BodyPart::Shoulders, 0),
            (BodyPart::BicepsForearm, 0),
            (BodyPart::Triceps, 1),
            (BodyPart::Legs, 1),
            (BodyPart::Abs, 0),
            (BodyPart::Other, 0),
        ]
    );
    let total: u32 = view.body_part_days.iter().map(|entry| entry.days).sum();
    assert_eq!(total, 4, "the parts exceed the training days");
}

fn check_the_snapshot_body_part(server: &DevServer, cookie: &str) {
    let stored = first_row(
        server,
        &format!(
            "SELECT body_part, snapshot FROM training_exercises te \
             JOIN master_exercises me ON me.id = te.exercise_id \
             WHERE te.id = '{ROW_TRAINING_EXERCISE}'"
        ),
    );
    assert_eq!(
        stored["body_part"], "legs",
        "the master must disagree with the snapshot"
    );
    assert_eq!(
        serde_json::from_str::<Value>(
            stored["snapshot"]
                .as_str()
                .expect("the stored snapshot is text")
        )
        .expect("the stored snapshot is JSON")["body_part"],
        "back"
    );
    let view = month(server, cookie, "2026-01");
    let day = view
        .days
        .iter()
        .find(|day| day.date == JstDate::new(2026, 1, 20).unwrap())
        .expect("the day with the snapshot");
    assert_eq!(day.body_parts, vec![BodyPart::Back]);
}

fn check_the_month_boundary(server: &DevServer, cookie: &str) {
    let january = month(server, cookie, "2026-01");
    assert!(
        january
            .days
            .iter()
            .any(|day| day.date == JstDate::new(2026, 1, 31).unwrap()),
        "the last day of January belongs to January"
    );
    let february = month(server, cookie, "2026-02");
    assert_eq!(february.start, JstDate::new(2026, 2, 1).unwrap());
    assert_eq!(february.end, JstDate::new(2026, 2, 28).unwrap());
    assert_eq!(
        february
            .days
            .iter()
            .map(|day| (day.date.day(), day.body_parts.clone()))
            .collect::<Vec<_>>(),
        vec![(1, vec![BodyPart::Legs])]
    );
    assert_eq!(february.training_days, 1);
    let stored = first_row(
        server,
        &format!("SELECT date, started_at FROM training_sessions WHERE id = '{FEB_FIRST}'"),
    );
    assert_eq!(stored["date"], "2026-02-01");
    assert_eq!(stored["started_at"], "2026-01-31T15:00:00Z");
}

fn check_the_empty_month(server: &DevServer, cookie: &str) {
    let march = month(server, cookie, "2026-03");
    assert!(march.days.is_empty());
    assert_eq!(march.training_days, 0);
    assert_eq!(march.body_part_days.len(), 8);
    assert!(march.body_part_days.iter().all(|entry| entry.days == 0));
}

fn check_the_day_detail(server: &DevServer, cookie: &str) {
    let detail = get_day(server, cookie, "2026-01-10");
    assert_eq!(detail.date, JstDate::new(2026, 1, 10).unwrap());
    assert_eq!(detail.total_weight_kg, 1_900.0);
    assert_eq!(detail.trainings.len(), 1);
    let training = &detail.trainings[0];
    assert_eq!(training.training_id, JAN_FIRST);
    assert_eq!(training.started_at.as_deref(), Some("2026-01-10T03:00:00Z"));
    assert_eq!(training.total_weight_kg, 1_900.0);
    assert_eq!(
        training
            .exercises
            .iter()
            .map(|exercise| (exercise.exercise_id.as_str(), exercise.name.as_str()))
            .collect::<Vec<_>>(),
        vec![(BENCH, "ベンチプレス"), (PUSHDOWN, "プレスダウン")]
    );
    let bench = &training.exercises[0];
    assert_eq!(bench.body_part, BodyPart::Chest);
    assert_eq!(bench.record_mode, RecordMode::WeightAndReps);
    assert_eq!(bench.total_weight_kg, 1_540.0);
    // 総重量は補助の回数を除き、反動の回数を含める (PRD の「計算ルールと用語定義」)。
    assert_eq!(
        bench
            .sets
            .iter()
            .map(|set| (set.kind, set.total_weight_kg))
            .collect::<Vec<_>>(),
        vec![
            (SetKind::Warmup, 0.0),
            (SetKind::Normal, 400.0),
            (SetKind::Drop, 1_140.0),
        ]
    );
    assert_eq!(bench.sets[1].rpe, Some(8.0));
    assert_eq!(bench.sets[1].grip.as_deref(), Some("pronated_medium"));
    assert_eq!(bench.sets[1].segments[0].momentum_reps, 2);
    assert_eq!(bench.sets[1].segments[0].assisted_reps, 1);
    assert_eq!(
        bench.sets[1]
            .segments
            .iter()
            .map(|segment| (segment.weight_kg, segment.reps))
            .collect::<Vec<_>>(),
        vec![(100.0, 5)]
    );
    assert_eq!(
        bench.sets[2]
            .segments
            .iter()
            .map(|segment| (segment.weight_kg, segment.reps))
            .collect::<Vec<_>>(),
        vec![(90.0, 8), (70.0, 6)]
    );
    let pushdown = &training.exercises[1];
    assert_eq!(pushdown.body_part, BodyPart::Triceps);
    assert_eq!(pushdown.total_weight_kg, 360.0);
    assert_eq!(pushdown.sets.len(), 1);
    assert_eq!(pushdown.sets[0].segments[0].reps, 12);
}

fn check_the_day_orders_sessions_without_a_start(server: &DevServer, cookie: &str) {
    let detail = get_day(server, cookie, "2026-01-20");
    assert_eq!(
        detail
            .trainings
            .iter()
            .map(|training| (
                training.training_id.as_str(),
                training.started_at.as_deref()
            ))
            .collect::<Vec<_>>(),
        vec![
            (JAN_SECOND, Some("2026-01-20T03:00:00Z")),
            (JAN_SECOND_EMPTY, Some("2026-01-20T10:00:00Z")),
            (JAN_SECOND_WITHOUT_START, None),
        ],
        "a session without a start time is ordered by its creation time"
    );
}

fn check_the_day_without_training(server: &DevServer, cookie: &str) {
    let detail = get_day(server, cookie, "2026-01-11");
    assert!(detail.trainings.is_empty());
    assert_eq!(detail.total_weight_kg, 0.0);
}

fn check_rejections(server: &DevServer, cookie: &str) {
    for path in [
        "/api/calendar",
        "/api/calendar?month=2026-1",
        "/api/calendar?month=2026-13",
        "/api/calendar/2026-02-30",
    ] {
        let rejected = server.request("GET", path, &[("Cookie", cookie)]);
        assert_eq!(rejected.status, 400, "{path}: {}", rejected.body);
        assert!(
            rejected.body.contains("\"code\":\"bad_request\""),
            "{}",
            rejected.body
        );
    }
    for path in [
        "/api/calendar?month=2026-01",
        "/api/calendar/2026-01-10",
        "/api/calendar/2026-02-30",
    ] {
        let unauthorized = server.request("GET", path, &[]);
        assert_eq!(unauthorized.status, 401, "{path}: {}", unauthorized.body);
        assert!(
            unauthorized.body.contains("\"code\":\"unauthorized\""),
            "{}",
            unauthorized.body
        );
    }
}

fn month(server: &DevServer, cookie: &str, month: &str) -> CalendarMonth {
    let response = server.request(
        "GET",
        &format!("/api/calendar?month={month}"),
        &[("Cookie", cookie)],
    );
    assert_eq!(response.status, 200, "{}", response.body);
    serde_json::from_str(&response.body).expect("the month response")
}

fn get_day(server: &DevServer, cookie: &str, date: &str) -> CalendarDayDetail {
    let response = server.request(
        "GET",
        &format!("/api/calendar/{date}"),
        &[("Cookie", cookie)],
    );
    assert_eq!(response.status, 200, "{}", response.body);
    serde_json::from_str(&response.body).expect("the day response")
}

fn counts(view: &CalendarMonth) -> Vec<(BodyPart, u32)> {
    view.body_part_days
        .iter()
        .map(|entry| (entry.body_part, entry.days))
        .collect()
}

fn seed(server: &DevServer, token: &str) {
    let now = "2026-01-01T00:00:00Z";
    let mut statements = vec![
        format!(
            "INSERT INTO users (id, google_sub, email, display_name, created_at, updated_at) \
             VALUES ('{USER_ID}', 'sub-calendar', 'calendar@example.test', 'カレンダーの利用者', '{now}', '{now}')"
        ),
        format!(
            "INSERT INTO users (id, google_sub, email, display_name, created_at, updated_at) \
             VALUES ('{OTHER_USER_ID}', 'sub-other-calendar', 'other-calendar@example.test', '別の利用者', '{now}', '{now}')"
        ),
        format!(
            "INSERT INTO auth_sessions (id, user_id, token_hash, created_at, expires_at) \
             VALUES ('3f2504e0-4f89-41d3-9a0c-0305e82c4850', '{USER_ID}', '{}', '{now}', '2099-01-01T00:00:00Z')",
            hash_session_token(token)
        ),
        format!(
            "INSERT INTO master_exercises (id, name, name_kana, body_part, equipment, record_mode, \
             laterality, grip_mode, other_scope, instructions, cautions, tips, sort_order) \
             VALUES ('{ROW}', 'シーテッドロウ', 'しーてっどろう', 'legs', 'cable', 'weight_and_reps', \
             'bilateral', 'none', NULL, '', '', '', 1)"
        ),
        session_sql(OTHER_JAN, OTHER_USER_ID, "2026-01-11", "2026-01-11T03:00:00Z", "別の記録"),
        session_sql(JAN_FIRST, USER_ID, "2026-01-10", "2026-01-10T03:00:00Z", ""),
        session_sql(JAN_SECOND, USER_ID, "2026-01-20", "2026-01-20T03:00:00Z", ""),
        session_sql(
            JAN_SECOND_EMPTY,
            USER_ID,
            "2026-01-20",
            "2026-01-20T10:00:00Z",
            "同じ日の空の記録",
        ),
        session_without_start_sql(
            JAN_SECOND_WITHOUT_START,
            USER_ID,
            "2026-01-20",
            "2026-01-20T23:00:00Z",
            "後から追加した記録",
        ),
        session_sql(JAN_LAST, USER_ID, "2026-01-31", "2026-01-31T14:59:59Z", ""),
        session_sql(FEB_FIRST, USER_ID, "2026-02-01", "2026-01-31T15:00:00Z", ""),
        exercise_sql(
            JAN_FIRST,
            BENCH_TRAINING_EXERCISE,
            USER_ID,
            BENCH,
            "ベンチプレス",
            "chest",
            1,
        ),
        exercise_sql(
            JAN_FIRST,
            PUSHDOWN_TRAINING_EXERCISE,
            USER_ID,
            PUSHDOWN,
            "プレスダウン",
            "triceps",
            2,
        ),
        exercise_sql(
            JAN_SECOND,
            ROW_TRAINING_EXERCISE,
            USER_ID,
            ROW,
            "シーテッドロウ",
            "back",
            1,
        ),
        exercise_sql(
            JAN_LAST,
            SQUAT_TRAINING_EXERCISE,
            USER_ID,
            SQUAT,
            "スクワット",
            "legs",
            1,
        ),
        exercise_sql(
            FEB_FIRST,
            FEB_SQUAT_TRAINING_EXERCISE,
            USER_ID,
            SQUAT,
            "スクワット",
            "legs",
            1,
        ),
        exercise_sql(
            OTHER_JAN,
            OTHER_TRAINING_EXERCISE,
            OTHER_USER_ID,
            CRUNCH,
            "クランチ",
            "abs",
            1,
        ),
    ];
    statements.extend([
        set_sql(
            USER_ID,
            BENCH_TRAINING_EXERCISE,
            WARMUP_SET,
            1,
            "warmup",
            None,
        ),
        set_sql(
            USER_ID,
            BENCH_TRAINING_EXERCISE,
            NORMAL_SET,
            2,
            "normal",
            Some(BENCH_GRIP_JSON),
        ),
        set_sql(
            USER_ID,
            BENCH_TRAINING_EXERCISE,
            DROP_SET,
            3,
            "drop",
            Some(BENCH_GRIP_JSON),
        ),
        set_sql(
            USER_ID,
            PUSHDOWN_TRAINING_EXERCISE,
            PUSHDOWN_SET,
            1,
            "normal",
            None,
        ),
        set_sql(USER_ID, ROW_TRAINING_EXERCISE, ROW_SET, 1, "normal", None),
        set_sql(
            USER_ID,
            SQUAT_TRAINING_EXERCISE,
            SQUAT_SET,
            1,
            "normal",
            None,
        ),
        set_sql(
            USER_ID,
            FEB_SQUAT_TRAINING_EXERCISE,
            FEB_SQUAT_SET,
            1,
            "normal",
            None,
        ),
        set_sql(
            OTHER_USER_ID,
            OTHER_TRAINING_EXERCISE,
            CRUNCH_SET,
            1,
            "normal",
            None,
        ),
        segment_sql(
            USER_ID,
            WARMUP_SET,
            "3f2504e0-4f89-41d3-9a0c-0305e82c4860",
            1,
            40.0,
            10,
        ),
        segment_sql(
            USER_ID,
            NORMAL_SET,
            "3f2504e0-4f89-41d3-9a0c-0305e82c4861",
            1,
            100.0,
            5,
        ),
        segment_sql(
            USER_ID,
            DROP_SET,
            "3f2504e0-4f89-41d3-9a0c-0305e82c4862",
            1,
            90.0,
            8,
        ),
        segment_sql(
            USER_ID,
            DROP_SET,
            "3f2504e0-4f89-41d3-9a0c-0305e82c4863",
            2,
            70.0,
            6,
        ),
        segment_sql(
            USER_ID,
            PUSHDOWN_SET,
            "3f2504e0-4f89-41d3-9a0c-0305e82c4864",
            1,
            30.0,
            12,
        ),
        segment_sql(
            USER_ID,
            ROW_SET,
            "3f2504e0-4f89-41d3-9a0c-0305e82c4865",
            1,
            50.0,
            10,
        ),
        segment_sql(
            USER_ID,
            SQUAT_SET,
            "3f2504e0-4f89-41d3-9a0c-0305e82c4866",
            1,
            80.0,
            8,
        ),
        segment_sql(
            USER_ID,
            FEB_SQUAT_SET,
            "3f2504e0-4f89-41d3-9a0c-0305e82c4867",
            1,
            100.0,
            5,
        ),
        segment_sql(
            OTHER_USER_ID,
            CRUNCH_SET,
            "3f2504e0-4f89-41d3-9a0c-0305e82c4868",
            1,
            0.0,
            20,
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
    server
        .execute_d1_json(
            "UPDATE set_segments SET momentum_reps = 2, assisted_reps = 1 \
             WHERE id = '3f2504e0-4f89-41d3-9a0c-0305e82c4861'",
        )
        .expect("the momentum and assisted reps must be stored");
}

fn session_sql(id: &str, user_id: &str, date: &str, started_at: &str, memo: &str) -> String {
    format!(
        "INSERT INTO training_sessions (id, user_id, date, started_at, memo, created_at, updated_at) \
         VALUES ('{id}', '{user_id}', '{date}', '{started_at}', '{memo}', \
         '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"
    )
}

fn session_without_start_sql(
    id: &str,
    user_id: &str,
    date: &str,
    created_at: &str,
    memo: &str,
) -> String {
    format!(
        "INSERT INTO training_sessions (id, user_id, date, started_at, memo, created_at, updated_at) \
         VALUES ('{id}', '{user_id}', '{date}', NULL, '{memo}', '{created_at}', '{created_at}')"
    )
}

fn exercise_sql(
    training_id: &str,
    training_exercise_id: &str,
    user_id: &str,
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

fn first_row(server: &DevServer, sql: &str) -> Value {
    server
        .execute_d1_json(sql)
        .expect("the query must run")
        .get(0)
        .and_then(|entry| entry.get("results"))
        .and_then(|rows| rows.get(0))
        .cloned()
        .expect("a row")
}
