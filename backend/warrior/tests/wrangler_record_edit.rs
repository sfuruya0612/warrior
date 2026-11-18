//! 過去のトレーニングのセットの編集、トレーニングの削除、日付を指定した追加の結合テスト。
//!
//! 対応する要求: `REQ-REC-006`。
//! 終了したトレーニングの完了済みのセットを編集すると、保存した値と、残っている記録から
//! 計算し直した自己ベストが変わる。トレーニングを削除すると、履歴とカレンダーから消える。
//! 日付を指定して追加した記録は、開始時刻を持たず、指定した所要時間を持ち、その日の
//! トレーニングとしてカレンダーに現れる。別のアカウントの ID は 404 になる。
//! AI の評価と次回セットの提案は、この変更の経路を持たないため作り直されない。

#[allow(dead_code)]
#[path = "support/record_set.rs"]
mod helpers;
mod support;

use helpers::*;
use serde_json::{json, Value};
use support::{DevServer, HttpResponse};
use warrior_core::base64url;

const PORT: u16 = 8860;
const SECOND_PORT: u16 = 8861;

const EDIT_OPERATION_ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c5a01";
const LOWER_OPERATION_ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c5a02";
const EMPTY_EDIT_OPERATION_ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c5a03";
const DELETE_SET_OPERATION_ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c5a04";
const FOREIGN_EDIT_OPERATION_ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c5a05";
const FOREIGN_DELETE_OPERATION_ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c5a06";
const DELETE_TRAINING_OPERATION_ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c5a07";
const BACKFILL_OPERATION_ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c5a10";
const INVALID_DATE_OPERATION_ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c5a11";
const LONG_DURATION_OPERATION_ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c5a12";
const EMPTY_DURATION_OPERATION_ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c5a13";
const FOREIGN_BACKFILL_OPERATION_ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c5a14";
const BACKFILL_TRAINING_ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c5a20";
const SECOND_BACKFILL_TRAINING_ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c5a32";

#[test]
fn a_past_records_set_edit_and_delete_change_the_history_the_calendar_and_the_bests() {
    let server = DevServer::start(PORT);
    server
        .apply_migrations()
        .expect("the local D1 migrations must apply");
    let token = base64url::encode(&[81u8; 32]);
    let terminal_token = base64url::encode(&[82u8; 32]);
    seed_base(
        &server,
        &token,
        &terminal_token,
        &base64url::encode(&[83u8; 32]),
    );
    let cookie = cookie(&token, &terminal_token);
    assert_eq!(best_count(&server), 0);

    let edit = update_operation(
        EDIT_OPERATION_ID,
        FINISHED_TRAINING_ID,
        FINISHED_SET_ID,
        UpdateSpec {
            position: 1,
            kind: "normal",
            completed: true,
            body_weight_kg: None,
        },
        vec![segment(FINISHED_SEGMENT_ID, 1, 80.0, 5)],
    );
    assert_applied(&post(&server, &cookie, &edit));
    assert_eq!(segment_weight(&server, FINISHED_SEGMENT_ID), 80.0);
    let bests = best_rows(&server);
    assert_eq!(bests.len(), 3, "{bests:?}");
    assert_best(&bests, "max_weight", 80.0);
    assert_best(&bests, "total_weight", 400.0);
    let day = calendar_day(&server, &cookie, "2026-11-11");
    assert_eq!(day["trainings"][0]["training_id"], FINISHED_TRAINING_ID);
    assert_eq!(day["total_weight_kg"].as_f64(), Some(400.0));
    assert_eq!(
        day["trainings"][0]["exercises"][0]["sets"][0]["total_weight_kg"].as_f64(),
        Some(400.0)
    );

    let lower = update_operation(
        LOWER_OPERATION_ID,
        FINISHED_TRAINING_ID,
        FINISHED_SET_ID,
        UpdateSpec {
            position: 1,
            kind: "normal",
            completed: true,
            body_weight_kg: None,
        },
        vec![segment(FINISHED_SEGMENT_ID, 1, 40.0, 5)],
    );
    assert_applied(&post(&server, &cookie, &lower));
    let bests = best_rows(&server);
    assert_eq!(bests.len(), 3, "{bests:?}");
    assert_best(&bests, "max_weight", 40.0);
    assert_best(&bests, "total_weight", 200.0);
    assert_eq!(
        calendar_day(&server, &cookie, "2026-11-11")["total_weight_kg"].as_f64(),
        Some(200.0)
    );

    let empty = update_operation(
        EMPTY_EDIT_OPERATION_ID,
        FINISHED_TRAINING_ID,
        FINISHED_SET_ID,
        UpdateSpec {
            position: 1,
            kind: "normal",
            completed: true,
            body_weight_kg: None,
        },
        Vec::new(),
    );
    let response = post(&server, &cookie, &empty);
    assert_eq!(response.status, 400, "{}", response.body);
    assert_eq!(applied_count(&server, EMPTY_EDIT_OPERATION_ID), 0);
    assert_eq!(segment_weight(&server, FINISHED_SEGMENT_ID), 40.0);

    let foreign_edit = update_operation(
        FOREIGN_EDIT_OPERATION_ID,
        OTHER_TRAINING_ID,
        OTHER_SET_ID,
        UpdateSpec {
            position: 1,
            kind: "normal",
            completed: true,
            body_weight_kg: None,
        },
        vec![segment(OTHER_SEGMENT_ID, 1, 999.0, 5)],
    );
    let response = post(&server, &cookie, &foreign_edit);
    assert_eq!(response.status, 404, "{}", response.body);
    assert_eq!(segment_weight(&server, OTHER_SEGMENT_ID), 60.0);
    assert_eq!(applied_count(&server, FOREIGN_EDIT_OPERATION_ID), 0);

    assert_applied(&post(
        &server,
        &cookie,
        &delete_operation(
            DELETE_SET_OPERATION_ID,
            FINISHED_TRAINING_ID,
            FINISHED_SET_ID,
        ),
    ));
    assert_eq!(count(&server, "sets", FINISHED_SET_ID), 0);
    assert_eq!(count(&server, "set_segments", FINISHED_SEGMENT_ID), 0);
    assert_eq!(best_count(&server), 0, "a best without a record is removed");
    let day = calendar_day(&server, &cookie, "2026-11-11");
    assert_eq!(day["total_weight_kg"].as_f64(), Some(0.0));
    assert_eq!(
        day["trainings"][0]["exercises"].as_array().map(Vec::len),
        Some(1),
        "the exercise remains after its last set is deleted"
    );
    assert_eq!(
        day["trainings"][0]["exercises"][0]["exercise_id"],
        BENCH_EXERCISE_ID
    );
    assert_eq!(
        day["trainings"][0]["exercises"][0]["sets"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );

    let foreign_delete = json!({
        "operation_id": FOREIGN_DELETE_OPERATION_ID,
        "training_id": OTHER_TRAINING_ID,
        "sequence": 7,
        "kind": "training_session_delete",
        "parent_operation_id": null,
        "payload": { "training_id": OTHER_TRAINING_ID }
    });
    let response = post(&server, &cookie, &foreign_delete);
    assert_eq!(response.status, 404, "{}", response.body);
    assert_eq!(session_count(&server, OTHER_TRAINING_ID), 1);
    assert_eq!(applied_count(&server, FOREIGN_DELETE_OPERATION_ID), 0);

    let delete_training = json!({
        "operation_id": DELETE_TRAINING_OPERATION_ID,
        "training_id": FINISHED_TRAINING_ID,
        "sequence": 8,
        "kind": "training_session_delete",
        "parent_operation_id": null,
        "payload": { "training_id": FINISHED_TRAINING_ID }
    });
    assert_applied(&post(&server, &cookie, &delete_training));
    assert_eq!(session_count(&server, FINISHED_TRAINING_ID), 0);
    assert_eq!(count(&server, "training_exercises", FINISHED_ROW_ID), 0);
    assert_eq!(
        calendar_day(&server, &cookie, "2026-11-11")["trainings"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );
    let history = server.request(
        "GET",
        &format!("/api/trainings/{FINISHED_TRAINING_ID}"),
        &[("Origin", ORIGIN), ("Cookie", &cookie)],
    );
    assert_eq!(history.status, 404, "{}", history.body);
    let replay = post(&server, &cookie, &delete_training);
    assert_eq!(replay.status, 200, "{}", replay.body);
    assert_eq!(status(&replay), "already_applied");
}

#[test]
fn a_record_added_with_a_date_has_no_start_time_and_appears_in_the_calendar() {
    let server = DevServer::start(SECOND_PORT);
    server
        .apply_migrations()
        .expect("the local D1 migrations must apply");
    let token = base64url::encode(&[84u8; 32]);
    let terminal_token = base64url::encode(&[85u8; 32]);
    seed_base(
        &server,
        &token,
        &terminal_token,
        &base64url::encode(&[86u8; 32]),
    );
    let cookie = cookie(&token, &terminal_token);

    let backfill = backfill_operation(
        BACKFILL_OPERATION_ID,
        BACKFILL_TRAINING_ID,
        "2026-11-20",
        1_800,
    );
    assert_applied(&post(&server, &cookie, &backfill));
    let row = first_row(
        &server,
        &format!(
            "SELECT date, started_at, finished_at, duration_seconds FROM training_sessions \
             WHERE id = '{BACKFILL_TRAINING_ID}'"
        ),
    );
    assert_eq!(row["date"], "2026-11-20");
    assert!(row["started_at"].is_null(), "{row:?}");
    assert!(!row["finished_at"].is_null(), "{row:?}");
    assert_eq!(row["duration_seconds"], 1_800);
    let month = calendar_month(&server, &cookie, "2026-11");
    assert!(
        month["days"]
            .as_array()
            .expect("the days")
            .iter()
            .any(|day| day["date"] == "2026-11-20"),
        "{month:?}"
    );
    let day = calendar_day(&server, &cookie, "2026-11-20");
    assert_eq!(day["trainings"][0]["training_id"], BACKFILL_TRAINING_ID);
    assert!(day["trainings"][0]["started_at"].is_null(), "{day:?}");

    let replay = post(&server, &cookie, &backfill);
    assert_eq!(replay.status, 200, "{}", replay.body);
    assert_eq!(status(&replay), "already_applied");

    for (operation_id, date, duration_seconds) in [
        (INVALID_DATE_OPERATION_ID, "2026-02-30", 1_800),
        (LONG_DURATION_OPERATION_ID, "2026-11-21", 86_401),
        (EMPTY_DURATION_OPERATION_ID, "2026-11-21", 0),
    ] {
        let invalid = backfill_operation(
            operation_id,
            SECOND_BACKFILL_TRAINING_ID,
            date,
            duration_seconds,
        );
        let response = post(&server, &cookie, &invalid);
        assert_eq!(response.status, 400, "{}", response.body);
        assert_eq!(applied_count(&server, operation_id), 0);
        assert_eq!(session_count(&server, SECOND_BACKFILL_TRAINING_ID), 0);
    }

    let foreign = backfill_operation(
        FOREIGN_BACKFILL_OPERATION_ID,
        OTHER_TRAINING_ID,
        "2026-11-22",
        1_800,
    );
    let response = post(&server, &cookie, &foreign);
    assert_eq!(response.status, 404, "{}", response.body);
    assert_eq!(applied_count(&server, FOREIGN_BACKFILL_OPERATION_ID), 0);
    let other = first_row(
        &server,
        &format!("SELECT date, user_id FROM training_sessions WHERE id = '{OTHER_TRAINING_ID}'"),
    );
    assert_eq!(other["user_id"], OTHER_USER_ID);
    assert_eq!(other["date"], "2026-11-12");
}

fn assert_applied(response: &HttpResponse) {
    assert_eq!(response.status, 200, "{}", response.body);
    assert_eq!(status(response), "applied");
}

fn backfill_operation(
    operation_id: &str,
    training_id: &str,
    date: &str,
    duration_seconds: i64,
) -> Value {
    json!({
        "operation_id": operation_id,
        "training_id": training_id,
        "sequence": 1,
        "kind": "training_session_backfill",
        "parent_operation_id": null,
        "payload": {
            "id": training_id,
            "date": date,
            "duration_seconds": duration_seconds,
            "memo": ""
        }
    })
}

fn segment_weight(server: &DevServer, segment_id: &str) -> f64 {
    first_row(
        server,
        &format!("SELECT weight_kg FROM set_segments WHERE id = '{segment_id}'"),
    )["weight_kg"]
        .as_f64()
        .expect("a stored weight")
}

fn session_count(server: &DevServer, training_id: &str) -> i64 {
    count_with(
        server,
        &format!("SELECT COUNT(*) AS count FROM training_sessions WHERE id = '{training_id}'"),
    )
}

fn best_count(server: &DevServer) -> i64 {
    count_with(
        server,
        &format!("SELECT COUNT(*) AS count FROM personal_bests WHERE user_id = '{USER_ID}'"),
    )
}

fn best_rows(server: &DevServer) -> Vec<Value> {
    server
        .execute_d1_json(&format!(
            "SELECT exercise_id, grip, attachment, kind, value, achieved_on, training_id \
             FROM personal_bests WHERE user_id = '{USER_ID}' ORDER BY kind"
        ))
        .expect("the personal bests must be readable")
        .get(0)
        .and_then(|entry| entry.get("results"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn assert_best(rows: &[Value], kind: &str, expected: f64) {
    let row = rows
        .iter()
        .find(|row| row["kind"] == kind)
        .unwrap_or_else(|| panic!("the {kind} row must exist: {rows:?}"));
    assert_eq!(row["exercise_id"], BENCH_EXERCISE_ID);
    assert_eq!(row["grip"], "");
    assert_eq!(row["attachment"], "");
    assert_eq!(row["training_id"], FINISHED_TRAINING_ID);
    assert_eq!(row["achieved_on"], "2026-11-11");
    let value = row["value"].as_f64().expect("a value");
    assert!(
        (value - expected).abs() < 1e-9,
        "{kind} must be {expected} but was {value}"
    );
}

fn calendar_month(server: &DevServer, cookie: &str, month: &str) -> Value {
    let response = server.request(
        "GET",
        &format!("/api/calendar?month={month}"),
        &[("Origin", ORIGIN), ("Cookie", cookie)],
    );
    assert_eq!(response.status, 200, "{}", response.body);
    serde_json::from_str(&response.body).expect("the calendar month is JSON")
}

fn calendar_day(server: &DevServer, cookie: &str, date: &str) -> Value {
    let response = server.request(
        "GET",
        &format!("/api/calendar/{date}"),
        &[("Origin", ORIGIN), ("Cookie", cookie)],
    );
    assert_eq!(response.status, 200, "{}", response.body);
    serde_json::from_str(&response.body).expect("the calendar day is JSON")
}
