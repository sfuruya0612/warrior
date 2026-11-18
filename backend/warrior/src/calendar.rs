//! カレンダーの月の集計と、日の詳細 (PRD の「カレンダー」)。
//!
//! 月の境界は日本時間で決め、日ごとの部位は記録時のスナップショットから取る (ADR-005、ADR-006)。
//! 集計は `warrior_core::calendar` が行い、このモジュールは D1 の行を渡して応答を組み立てる (ADR-016)。

use std::collections::HashMap;

use serde::Deserialize;
use warrior_core::calendar as core_calendar;
use warrior_core::calendar::{
    CalendarDayDetail, CalendarExercise, CalendarMonth, CalendarTraining,
};
use warrior_core::exercise::{BodyPart, ExerciseSnapshot};
use warrior_core::jst::{JstDate, JstMonth};
use warrior_core::sql::{SqlStatement, SqlValue};
use worker::{console_error, Request, Response, Result, RouteContext};

use crate::auth::session;
use crate::db::Db;
use crate::error::{response, ErrorCode};
use crate::stored_sets::{self, StoredExercise};

#[derive(Debug, Deserialize)]
struct DateRow {
    date: String,
}

#[derive(Debug, Deserialize)]
struct SnapshotRow {
    date: String,
    snapshot: String,
}

#[derive(Debug, Deserialize)]
struct SessionRow {
    id: String,
    started_at: Option<String>,
    memo: String,
}

#[derive(Debug, Deserialize)]
struct PlannedRow {
    training_id: String,
    snapshot: String,
}

/// 保存した行をカレンダーの表示に変換できない理由。
#[derive(Debug, Clone, PartialEq, Eq)]
enum StoredCalendarError {
    /// 保存した日付が日本時間の日付でない。
    InvalidDate(String),
    /// 種目のスナップショットを復元できない。
    UnreadableSnapshot,
}

/// `GET /api/calendar?month=YYYY-MM`
pub async fn month(request: Request, context: RouteContext<()>) -> Result<Response> {
    let env = context.env;
    let Some(authenticated) = session::resolve(&request, &env).await? else {
        return Ok(response(ErrorCode::Unauthorized, "authentication required"));
    };
    let query = request
        .url()?
        .query_pairs()
        .into_owned()
        .collect::<HashMap<_, _>>();
    let Some(month) = query
        .get("month")
        .and_then(|text| core_calendar::parse_month(text))
    else {
        return Ok(response(ErrorCode::BadRequest, "the month must be YYYY-MM"));
    };
    let db = Db::new(env.d1("DB")?);
    let dates = db
        .all(&month_dates_statement(&authenticated.id, month))
        .await?
        .results::<DateRow>()?;
    let snapshots = db
        .all(&month_snapshots_statement(&authenticated.id, month))
        .await?
        .results::<SnapshotRow>()?;
    match build_month(month, dates, snapshots) {
        Ok(view) => Response::from_json(&view),
        Err(error) => {
            console_error!("the stored calendar rows are invalid: {error:?}");
            Ok(response(ErrorCode::Internal, "internal error"))
        }
    }
}

/// `GET /api/calendar/:date`
pub async fn day(request: Request, context: RouteContext<()>) -> Result<Response> {
    // 認証を日付の検証より先に返すため、param は env の取り出し前に読み取る。
    let date_text = context.param("date").cloned();
    let env = context.env;
    let Some(authenticated) = session::resolve(&request, &env).await? else {
        return Ok(response(ErrorCode::Unauthorized, "authentication required"));
    };
    let Some(text) = date_text else {
        return Ok(response(ErrorCode::NotFound, "resource not found"));
    };
    let Some(date) = JstDate::parse(&text) else {
        return Ok(response(
            ErrorCode::BadRequest,
            "the date must be YYYY-MM-DD",
        ));
    };
    let db = Db::new(env.d1("DB")?);
    let sessions = db
        .all(&day_sessions_statement(&authenticated.id, date))
        .await?
        .results::<SessionRow>()?;
    let exercises = match stored_sets::load_day(&db, &authenticated.id, &date.to_string()).await {
        Ok(exercises) => exercises,
        Err(error) => {
            console_error!("failed to build the day sets: {error}");
            return Ok(response(ErrorCode::Internal, "internal error"));
        }
    };
    let planned = db
        .all(&day_planned_statement(&authenticated.id, date))
        .await?
        .results::<PlannedRow>()?;
    Response::from_json(&build_day(date, sessions, exercises, planned))
}

fn month_dates_statement(user_id: &str, month: JstMonth) -> SqlStatement {
    SqlStatement::new(
        "SELECT DISTINCT date FROM training_sessions \
         WHERE user_id = ? AND date >= ? AND date <= ? ORDER BY date",
        month_bindings(user_id, month),
    )
}

fn month_snapshots_statement(user_id: &str, month: JstMonth) -> SqlStatement {
    SqlStatement::new(
        "SELECT ts.date AS date, te.snapshot AS snapshot \
         FROM training_sessions ts \
         JOIN training_exercises te ON te.training_id = ts.id AND te.user_id = ts.user_id \
         WHERE ts.user_id = ? AND ts.date >= ? AND ts.date <= ? \
         ORDER BY ts.date, te.position",
        month_bindings(user_id, month),
    )
}

fn month_bindings(user_id: &str, month: JstMonth) -> Vec<SqlValue> {
    vec![
        SqlValue::Text(user_id.to_owned()),
        SqlValue::Text(month.start().to_string()),
        SqlValue::Text(month.end().to_string()),
    ]
}

fn day_sessions_statement(user_id: &str, date: JstDate) -> SqlStatement {
    SqlStatement::new(
        "SELECT id, started_at, memo FROM training_sessions \
         WHERE user_id = ? AND date = ? \
         ORDER BY COALESCE(started_at, created_at), id",
        vec![
            SqlValue::Text(user_id.to_owned()),
            SqlValue::Text(date.to_string()),
        ],
    )
}

fn day_planned_statement(user_id: &str, date: JstDate) -> SqlStatement {
    SqlStatement::new(
        "SELECT te.training_id, te.snapshot FROM training_sessions ts \
         JOIN training_exercises te ON te.training_id = ts.id AND te.user_id = ts.user_id \
         WHERE ts.user_id = ? AND ts.date = ? ORDER BY te.position, te.id",
        vec![
            SqlValue::Text(user_id.to_owned()),
            SqlValue::Text(date.to_string()),
        ],
    )
}

fn build_month(
    month: JstMonth,
    dates: Vec<DateRow>,
    snapshots: Vec<SnapshotRow>,
) -> std::result::Result<CalendarMonth, StoredCalendarError> {
    let mut parts_by_date = HashMap::<String, Vec<BodyPart>>::new();
    for row in snapshots {
        let snapshot = serde_json::from_str::<ExerciseSnapshot>(&row.snapshot)
            .map_err(|_| StoredCalendarError::UnreadableSnapshot)?;
        parts_by_date
            .entry(row.date)
            .or_default()
            .push(snapshot.body_part);
    }
    let mut days = Vec::new();
    for row in dates {
        let date = JstDate::parse(&row.date)
            .ok_or_else(|| StoredCalendarError::InvalidDate(row.date.clone()))?;
        let parts = parts_by_date.remove(&row.date).unwrap_or_default();
        days.push(core_calendar::TrainingDay {
            date,
            body_parts: core_calendar::day_body_parts(parts),
        });
    }
    Ok(core_calendar::month_view(month, days))
}

fn build_day(
    date: JstDate,
    sessions: Vec<SessionRow>,
    exercises: Vec<StoredExercise>,
    planned: Vec<PlannedRow>,
) -> CalendarDayDetail {
    let planned = planned
        .into_iter()
        .filter_map(|row| {
            serde_json::from_str::<ExerciseSnapshot>(&row.snapshot)
                .ok()
                .map(|snapshot| (row.training_id, snapshot))
        })
        .collect::<Vec<_>>();
    let trainings = sessions
        .into_iter()
        .map(|session| calendar_training(&session, &exercises, &planned))
        .collect::<Vec<_>>();
    let total_weight_kg = trainings
        .iter()
        .map(|training| training.total_weight_kg)
        .sum();
    CalendarDayDetail {
        date,
        total_weight_kg,
        trainings,
    }
}

fn calendar_training(
    session: &SessionRow,
    exercises: &[StoredExercise],
    planned: &[(String, ExerciseSnapshot)],
) -> CalendarTraining {
    let recorded = exercises
        .iter()
        .filter(|exercise| exercise.training_id == session.id)
        .collect::<Vec<_>>();
    let mut visible = Vec::new();
    let mut used = vec![false; recorded.len()];
    for (_, snapshot) in planned
        .iter()
        .filter(|(training_id, _)| training_id == &session.id)
    {
        if let Some(index) = recorded
            .iter()
            .position(|exercise| exercise.snapshot.exercise_id == snapshot.exercise_id)
        {
            used[index] = true;
            visible.push(calendar_exercise(recorded[index]));
        } else {
            visible.push(planned_exercise(snapshot));
        }
    }
    for (index, exercise) in recorded.into_iter().enumerate() {
        if !used[index] {
            visible.push(calendar_exercise(exercise));
        }
    }
    let total_weight_kg = visible
        .iter()
        .map(|exercise| exercise.total_weight_kg)
        .sum();
    CalendarTraining {
        training_id: session.id.clone(),
        started_at: session.started_at.clone(),
        memo: session.memo.clone(),
        total_weight_kg,
        exercises: visible,
    }
}

fn calendar_exercise(exercise: &StoredExercise) -> CalendarExercise {
    let sets = exercise
        .sets
        .iter()
        .map(core_calendar::set_detail)
        .collect::<Vec<_>>();
    let total_weight_kg = sets.iter().map(|set| set.total_weight_kg).sum();
    CalendarExercise {
        exercise_id: exercise.snapshot.exercise_id.clone(),
        name: exercise.snapshot.name.clone(),
        body_part: exercise.snapshot.body_part,
        record_mode: exercise.snapshot.record_mode,
        laterality: exercise.snapshot.laterality,
        total_weight_kg,
        sets,
    }
}

fn planned_exercise(snapshot: &ExerciseSnapshot) -> CalendarExercise {
    CalendarExercise {
        exercise_id: snapshot.exercise_id.clone(),
        name: snapshot.name.clone(),
        body_part: snapshot.body_part,
        record_mode: snapshot.record_mode,
        laterality: snapshot.laterality,
        total_weight_kg: 0.0,
        sets: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    //! サーバーの月の組み立てと日の詳細の例示テスト。
    //!
    //! 要求の受入基準は結合テストと E2E が担い、ここは D1 の行から応答への変換を確かめる。

    use warrior_core::exercise::{BodyPart, ExerciseSnapshot, GripMode, Laterality};
    use warrior_core::jst::JstDate;
    use warrior_core::set_calc::{BestKey, RecordMode, SetKind, SetRecord, SetSegment};
    use warrior_core::sql::SqlValue;

    use super::{
        build_day, build_month, day_sessions_statement, month_dates_statement,
        month_snapshots_statement, DateRow, PlannedRow, SessionRow, SnapshotRow,
        StoredCalendarError,
    };
    use crate::stored_sets::StoredExercise;

    #[test]
    fn the_month_statements_scope_the_user_and_the_range() {
        let month = JstDate::new(2026, 11, 12).unwrap().month_range();
        let dates = month_dates_statement("user-a", month);
        assert!(dates
            .sql()
            .contains("WHERE user_id = ? AND date >= ? AND date <= ?"));
        assert_eq!(
            dates.bindings(),
            [
                SqlValue::Text("user-a".to_owned()),
                SqlValue::Text("2026-11-01".to_owned()),
                SqlValue::Text("2026-11-30".to_owned()),
            ]
        );
        let snapshots = month_snapshots_statement("user-a", month);
        assert!(snapshots
            .sql()
            .contains("te.training_id = ts.id AND te.user_id = ts.user_id"));
        assert_eq!(snapshots.bindings(), dates.bindings());
    }

    #[test]
    fn the_day_statement_scopes_the_user_and_the_date() {
        let date = JstDate::new(2026, 11, 12).unwrap();
        let statement = day_sessions_statement("user-a", date);
        assert!(statement.sql().contains("WHERE user_id = ? AND date = ?"));
        assert_eq!(
            statement.bindings(),
            [
                SqlValue::Text("user-a".to_owned()),
                SqlValue::Text("2026-11-12".to_owned()),
            ]
        );
    }

    #[test]
    fn the_month_rows_become_the_days_and_the_counts() {
        let month = JstDate::new(2026, 11, 12).unwrap().month_range();
        let dates = vec![row("2026-11-01"), row("2026-11-03")];
        let snapshots = vec![
            snapshot("2026-11-01", BodyPart::Chest),
            snapshot("2026-11-01", BodyPart::Chest),
            snapshot("2026-11-01", BodyPart::Triceps),
            snapshot("2026-11-03", BodyPart::Legs),
        ];
        let view = build_month(month, dates, snapshots).expect("a month view");
        assert_eq!(view.month, "2026-11");
        assert_eq!(view.training_days, 2);
        assert_eq!(
            view.days
                .iter()
                .map(|day| (day.date.day(), day.body_parts.clone()))
                .collect::<Vec<_>>(),
            vec![
                (1, vec![BodyPart::Chest, BodyPart::Triceps]),
                (3, vec![BodyPart::Legs]),
            ]
        );
        let chest = view
            .body_part_days
            .iter()
            .find(|entry| entry.body_part == BodyPart::Chest)
            .expect("the chest count");
        assert_eq!(chest.days, 1);
        let total: u32 = view.body_part_days.iter().map(|entry| entry.days).sum();
        assert_eq!(total, 3, "the parts exceed the training days");
    }

    #[test]
    fn an_invalid_stored_date_is_rejected() {
        let month = JstDate::new(2026, 2, 1).unwrap().month_range();
        assert_eq!(
            build_month(month, vec![row("2026-02-30")], Vec::new()),
            Err(StoredCalendarError::InvalidDate("2026-02-30".to_owned()))
        );
    }

    #[test]
    fn an_unreadable_snapshot_is_rejected() {
        let month = JstDate::new(2026, 2, 1).unwrap().month_range();
        let snapshots = vec![SnapshotRow {
            date: "2026-02-01".to_owned(),
            snapshot: "not-json".to_owned(),
        }];
        assert_eq!(
            build_month(month, vec![row("2026-02-01")], snapshots),
            Err(StoredCalendarError::UnreadableSnapshot)
        );
    }

    #[test]
    fn the_day_detail_uses_the_set_calc_totals() {
        let date = JstDate::new(2026, 11, 12).unwrap();
        let sessions = vec![
            SessionRow {
                id: "training-b".to_owned(),
                started_at: None,
                memo: "夜".to_owned(),
            },
            SessionRow {
                id: "training-a".to_owned(),
                started_at: Some("2026-11-12T03:00:00Z".to_owned()),
                memo: String::new(),
            },
        ];
        let exercises = vec![
            StoredExercise {
                training_id: "training-b".to_owned(),
                snapshot: snapshot_of("plank", "プランク", BodyPart::Abs, RecordMode::Time),
                memo: String::new(),
                sets: vec![time_record("plank", 60)],
                set_grips: vec![None],
                set_body_weights: vec![None],
            },
            StoredExercise {
                training_id: "training-a".to_owned(),
                snapshot: snapshot_of(
                    "bench",
                    "ベンチプレス",
                    BodyPart::Chest,
                    RecordMode::WeightAndReps,
                ),
                memo: String::new(),
                sets: vec![
                    weight_record("bench", SetKind::Warmup, 60.0, 10),
                    weight_record("bench", SetKind::Normal, 100.0, 5),
                ],
                set_grips: vec![None, None],
                set_body_weights: vec![None, None],
            },
        ];
        let detail = build_day(date, sessions, exercises, Vec::new());
        assert_eq!(detail.date, date);
        assert_eq!(
            detail
                .trainings
                .iter()
                .map(|training| training.training_id.as_str())
                .collect::<Vec<_>>(),
            vec!["training-b", "training-a"]
        );
        assert_eq!(detail.trainings[0].total_weight_kg, 0.0);
        assert_eq!(detail.trainings[1].total_weight_kg, 500.0);
        assert_eq!(detail.total_weight_kg, 500.0);
        let bench = &detail.trainings[1].exercises[0];
        assert_eq!(bench.name, "ベンチプレス");
        assert_eq!(bench.body_part, BodyPart::Chest);
        assert_eq!(bench.record_mode, RecordMode::WeightAndReps);
        assert_eq!(bench.sets.len(), 2);
        assert_eq!(bench.sets[0].kind, SetKind::Warmup);
        assert_eq!(bench.sets[0].total_weight_kg, 0.0);
        assert_eq!(bench.sets[1].total_weight_kg, 500.0);
        assert_eq!(bench.sets[1].segments[0].reps, 5);
    }

    #[test]
    fn a_planned_exercise_without_sets_is_shown_with_an_empty_set_list() {
        let date = JstDate::new(2026, 11, 12).unwrap();
        let plan = PlannedRow {
            training_id: "training-a".to_owned(),
            snapshot: serde_json::to_string(&snapshot_of(
                "bench",
                "ベンチプレス",
                BodyPart::Chest,
                RecordMode::WeightAndReps,
            ))
            .expect("the snapshot is JSON"),
        };
        let detail = build_day(
            date,
            vec![SessionRow {
                id: "training-a".to_owned(),
                started_at: None,
                memo: String::new(),
            }],
            Vec::new(),
            vec![plan],
        );
        assert_eq!(detail.trainings[0].exercises.len(), 1);
        assert_eq!(detail.trainings[0].exercises[0].name, "ベンチプレス");
        assert!(detail.trainings[0].exercises[0].sets.is_empty());
    }

    #[test]
    fn a_day_without_sessions_has_no_trainings() {
        let date = JstDate::new(2026, 11, 12).unwrap();
        let detail = build_day(date, Vec::new(), Vec::new(), Vec::new());
        assert_eq!(detail.total_weight_kg, 0.0);
        assert!(detail.trainings.is_empty());
    }

    fn row(date: &str) -> DateRow {
        DateRow {
            date: date.to_owned(),
        }
    }

    fn snapshot(date: &str, body_part: BodyPart) -> SnapshotRow {
        SnapshotRow {
            date: date.to_owned(),
            snapshot: serde_json::to_string(&snapshot_of(
                "exercise",
                "種目",
                body_part,
                RecordMode::WeightAndReps,
            ))
            .expect("a serializable snapshot"),
        }
    }

    fn snapshot_of(
        exercise_id: &str,
        name: &str,
        body_part: BodyPart,
        record_mode: RecordMode,
    ) -> ExerciseSnapshot {
        ExerciseSnapshot {
            exercise_id: exercise_id.to_owned(),
            name: name.to_owned(),
            body_part,
            muscles: Vec::new(),
            record_mode,
            laterality: Laterality::Bilateral,
            grip_mode: GripMode::None,
            grip_options: Vec::new(),
        }
    }

    fn key(exercise_id: &str) -> BestKey {
        BestKey {
            exercise_id: exercise_id.to_owned(),
            grip: None,
            attachment: None,
        }
    }

    fn weight_record(exercise_id: &str, kind: SetKind, weight_kg: f64, reps: u32) -> SetRecord {
        SetRecord::new(
            key(exercise_id),
            kind,
            vec![SetSegment {
                weight_kg,
                reps,
                momentum_reps: 0,
                assisted_reps: 0,
                left: None,
                right: None,
                duration_seconds: None,
            }],
            None,
            None,
            RecordMode::WeightAndReps,
        )
        .expect("a valid weight set")
    }

    fn time_record(exercise_id: &str, seconds: u32) -> SetRecord {
        SetRecord::new(
            key(exercise_id),
            SetKind::Normal,
            Vec::new(),
            None,
            Some(seconds),
            RecordMode::Time,
        )
        .expect("a valid time set")
    }
}
