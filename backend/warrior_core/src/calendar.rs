//! カレンダーの月表示、日ごとの部位、月の集計 (PRD の「カレンダー」)。
//!
//! 月は日本時間の 1 日 0 時に始まり、翌月 1 日 0 時の直前で終わる (ADR-005)。
//! 日ごとの部位は、記録した時点の種目のスナップショットが持つ部位区分から取る (ADR-006)。
//! 1 日に複数の部位を行うと、部位区分ごとの日数の合計はトレーニング日数を超える。
//! 総重量は `set_calc` の計算だけを使い、このモジュールで再実装しない。

use serde::{Deserialize, Serialize};

use crate::exercise::{BodyPart, Laterality};
use crate::jst::{JstDate, JstMonth};
use crate::set_calc::{RecordMode, SetKind, SetRecord, SetSegment};

/// 1 日のトレーニングと、その日に行った部位区分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrainingDay {
    /// 日本時間の日付。
    pub date: JstDate,
    /// その日に行った部位区分。8 区分の並びで重複しない。
    pub body_parts: Vec<BodyPart>,
}

/// カレンダーに表示する 1 日。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarDay {
    /// 日本時間の日付。
    pub date: JstDate,
    /// その日に行った部位区分。
    pub body_parts: Vec<BodyPart>,
}

/// 部位区分ごとの日数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BodyPartDayCount {
    /// 部位区分。
    pub body_part: BodyPart,
    /// その部位を行った日数。
    pub days: u32,
}

/// 月のトレーニング日数と部位区分ごとの日数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarMonthSummary {
    /// その月にトレーニングした日数。
    pub training_days: u32,
    /// 8 区分すべての日数。その部位を行わなかった区分も 0 で含む。
    pub body_part_days: Vec<BodyPartDayCount>,
}

/// 月のカレンダーの表示内容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarMonth {
    /// `YYYY-MM` の月。
    pub month: String,
    /// 月の開始日。
    pub start: JstDate,
    /// 月の終了日。
    pub end: JstDate,
    /// トレーニングした日。トレーニングのない日は含まない。
    pub days: Vec<CalendarDay>,
    /// その月にトレーニングした日数。
    pub training_days: u32,
    /// 8 区分すべての日数。
    pub body_part_days: Vec<BodyPartDayCount>,
}

/// 日の詳細に表示する 1 セット。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalendarSet {
    /// セット種別。
    pub kind: SetKind,
    /// RPE。
    pub rpe: Option<f64>,
    /// グリップ。選ばない種目は None。
    pub grip: Option<String>,
    /// アタッチメント。選ばない種目は None。
    pub attachment: Option<String>,
    /// 時間で記録するセットの合計秒数。
    pub duration_seconds: Option<u32>,
    /// 総重量 (kg)。
    pub total_weight_kg: f64,
    /// 段の列。ドロップセットは複数の段を持つ。
    pub segments: Vec<SetSegment>,
}

/// 日の詳細に表示する種目。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalendarExercise {
    /// 種目の ID。
    pub exercise_id: String,
    /// 記録時点の種目名。
    pub name: String,
    /// 記録時点の部位区分。
    pub body_part: BodyPart,
    /// 記録方式。
    pub record_mode: RecordMode,
    /// 左右別に行うか。
    pub laterality: Laterality,
    /// 種目の総重量 (kg)。
    pub total_weight_kg: f64,
    /// セットの列。
    pub sets: Vec<CalendarSet>,
}

/// 日の詳細に表示するトレーニング。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalendarTraining {
    /// トレーニングの ID。
    pub training_id: String,
    /// 開始時刻。後から追加したトレーニングは None。
    pub started_at: Option<String>,
    /// メモ。
    pub memo: String,
    /// トレーニングの総重量 (kg)。
    pub total_weight_kg: f64,
    /// 種目の列。
    pub exercises: Vec<CalendarExercise>,
}

/// 選んだ日の詳細。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalendarDayDetail {
    /// 日本時間の日付。
    pub date: JstDate,
    /// その日の総重量 (kg)。
    pub total_weight_kg: f64,
    /// その日のトレーニングの列。
    pub trainings: Vec<CalendarTraining>,
}

/// 記録したセットから、日の詳細に表示する 1 セットを作る。
pub fn set_detail(record: &SetRecord) -> CalendarSet {
    CalendarSet {
        kind: record.kind(),
        rpe: record.rpe(),
        grip: record.key().grip.clone(),
        attachment: record.key().attachment.clone(),
        duration_seconds: (record.mode() == RecordMode::Time)
            .then(|| record.total_duration_seconds()),
        total_weight_kg: record.total_weight_kg(),
        segments: record.segments().to_vec(),
    }
}

/// `YYYY-MM` の文字列から月を読む。
pub fn parse_month(text: &str) -> Option<JstMonth> {
    if text.len() != 7 {
        return None;
    }
    let first = JstDate::parse(&format!("{text}-01"))?;
    Some(first.month_range())
}

/// 月の開始日から終了日までの日付。
pub fn month_days(month: JstMonth) -> Vec<JstDate> {
    let mut days = Vec::new();
    let mut current = Some(month.start());
    while let Some(date) = current {
        if !month.contains(date) {
            break;
        }
        days.push(date);
        current = date.add_days(1);
    }
    days
}

/// 月表示のセルの並び。週の始まりを月曜に揃え、月の外のセルは None にする。
pub fn month_cells(month: JstMonth) -> Vec<Option<JstDate>> {
    let leading = usize::from(month.start().weekday().index());
    let mut cells = vec![None; leading];
    cells.extend(month_days(month).into_iter().map(Some));
    while cells.len() % 7 != 0 {
        cells.push(None);
    }
    cells
}

/// 前の月。表現できる範囲の外に出るときは None。
pub fn previous_month(month: JstMonth) -> Option<JstMonth> {
    Some(month.start().add_days(-1)?.month_range())
}

/// 次の月。表現できる範囲の外に出るときは None。
pub fn next_month(month: JstMonth) -> Option<JstMonth> {
    Some(month.end().add_days(1)?.month_range())
}

/// その日に行った部位区分。8 区分の並びで重複を除く。
pub fn day_body_parts(parts: impl IntoIterator<Item = BodyPart>) -> Vec<BodyPart> {
    let parts = parts.into_iter().collect::<Vec<_>>();
    BodyPart::ALL
        .into_iter()
        .filter(|part| parts.contains(part))
        .collect()
}

/// 月のトレーニング日数と部位区分ごとの日数を集計する。
///
/// 同じ日付が複数回現れても 1 日として数える。
pub fn summarize(days: &[TrainingDay]) -> CalendarMonthSummary {
    let mut dates = Vec::new();
    for day in days {
        if !dates.contains(&day.date) {
            dates.push(day.date);
        }
    }
    let body_part_days = BodyPart::ALL
        .into_iter()
        .map(|part| BodyPartDayCount {
            body_part: part,
            days: count_days(&dates, days, part),
        })
        .collect();
    CalendarMonthSummary {
        training_days: to_u32(dates.len()),
        body_part_days,
    }
}

/// 月の日ごとの部位から、月の表示を組み立てる。
pub fn month_view(month: JstMonth, mut days: Vec<TrainingDay>) -> CalendarMonth {
    days.sort_by_key(|day| day.date);
    let summary = summarize(&days);
    let days = days
        .into_iter()
        .map(|day| CalendarDay {
            date: day.date,
            body_parts: day.body_parts,
        })
        .collect();
    CalendarMonth {
        month: format!("{:04}-{:02}", month.start().year(), month.start().month()),
        start: month.start(),
        end: month.end(),
        days,
        training_days: summary.training_days,
        body_part_days: summary.body_part_days,
    }
}

fn count_days(dates: &[JstDate], days: &[TrainingDay], part: BodyPart) -> u32 {
    let count = dates
        .iter()
        .filter(|date| {
            days.iter()
                .any(|day| day.date == **date && day.body_parts.contains(&part))
        })
        .count();
    to_u32(count)
}

fn to_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    //! カレンダーの集計の例示テスト。
    //!
    //! 任意入力の性質は `backend/pbt/tests/calendar.rs` が担う。ここは境界と固定シナリオを確かめる。

    use super::{
        day_body_parts, month_cells, month_days, month_view, next_month, parse_month,
        previous_month, set_detail, summarize, CalendarDay, CalendarSet, TrainingDay,
    };
    use crate::exercise::BodyPart;
    use crate::jst::{JstDate, UtcInstant};
    use crate::set_calc::{BestKey, RecordMode, SetKind, SetRecord, SetSegment, SideValue};

    #[test]
    fn parse_month_reads_the_yyyy_mm_text() {
        let november = parse_month("2026-11").expect("a month");
        assert_eq!(november.start(), JstDate::new(2026, 11, 1).unwrap());
        assert_eq!(november.end(), JstDate::new(2026, 11, 30).unwrap());
        for text in [
            "2026-1",
            "2026-13",
            "2026-00",
            "202611",
            "2026/11",
            "2026-11-01",
            "abcd-11",
            "",
        ] {
            assert_eq!(parse_month(text), None, "{text}");
        }
    }

    #[test]
    fn month_days_cover_month_end_and_leap_years() {
        let january = month_days(JstDate::new(2026, 1, 10).unwrap().month_range());
        assert_eq!(january.len(), 31);
        assert_eq!(january[0], JstDate::new(2026, 1, 1).unwrap());
        assert_eq!(january[30], JstDate::new(2026, 1, 31).unwrap());

        let leap_february = month_days(JstDate::new(2024, 2, 1).unwrap().month_range());
        assert_eq!(leap_february.len(), 29);
        assert_eq!(leap_february[28], JstDate::new(2024, 2, 29).unwrap());

        let february = month_days(JstDate::new(2026, 2, 1).unwrap().month_range());
        assert_eq!(february.len(), 28);

        let april = month_days(JstDate::new(2026, 4, 1).unwrap().month_range());
        assert_eq!(april.len(), 30);

        let december = month_days(JstDate::new(2026, 12, 1).unwrap().month_range());
        assert_eq!(december.len(), 31);
        assert_eq!(december[30], JstDate::new(2026, 12, 31).unwrap());
    }

    #[test]
    fn month_cells_start_on_monday_and_fill_the_last_week() {
        let november = JstDate::new(2026, 11, 12).unwrap().month_range();
        let cells = month_cells(november);
        assert_eq!(cells.len(), 42);
        assert_eq!(&cells[..6], &[None; 6]);
        assert_eq!(cells[6], Some(JstDate::new(2026, 11, 1).unwrap()));
        assert_eq!(cells[35], Some(JstDate::new(2026, 11, 30).unwrap()));
        assert_eq!(&cells[36..], &[None; 6]);

        let february = JstDate::new(2027, 2, 1).unwrap().month_range();
        assert_eq!(february.start().weekday().index(), 0);
        let cells = month_cells(february);
        assert_eq!(cells.len(), 28);
        assert_eq!(cells[0], Some(JstDate::new(2027, 2, 1).unwrap()));
        assert_eq!(cells[27], Some(JstDate::new(2027, 2, 28).unwrap()));
    }

    #[test]
    fn month_boundary_switches_at_japan_midnight() {
        let last_january = UtcInstant::parse("2026-01-31T14:59:59Z")
            .unwrap()
            .jst_date()
            .unwrap()
            .month_range();
        assert_eq!(last_january.start(), JstDate::new(2026, 1, 1).unwrap());
        assert_eq!(last_january.end(), JstDate::new(2026, 1, 31).unwrap());

        let first_february = UtcInstant::parse("2026-01-31T15:00:00Z")
            .unwrap()
            .jst_date()
            .unwrap()
            .month_range();
        assert_eq!(first_february.start(), JstDate::new(2026, 2, 1).unwrap());
        assert_eq!(first_february.end(), JstDate::new(2026, 2, 28).unwrap());
    }

    #[test]
    fn previous_and_next_month_cross_the_year() {
        let january = JstDate::new(2026, 1, 1).unwrap().month_range();
        assert_eq!(
            previous_month(january).map(|month| month.start()),
            Some(JstDate::new(2025, 12, 1).unwrap())
        );
        let december = JstDate::new(2026, 12, 1).unwrap().month_range();
        assert_eq!(
            next_month(december).map(|month| month.start()),
            Some(JstDate::new(2027, 1, 1).unwrap())
        );
    }

    #[test]
    fn day_body_parts_are_unique_and_ordered() {
        assert_eq!(
            day_body_parts([BodyPart::Triceps, BodyPart::Chest, BodyPart::Triceps]),
            vec![BodyPart::Chest, BodyPart::Triceps]
        );
        assert_eq!(
            day_body_parts(BodyPart::ALL),
            vec![
                BodyPart::Chest,
                BodyPart::Back,
                BodyPart::Shoulders,
                BodyPart::BicepsForearm,
                BodyPart::Triceps,
                BodyPart::Legs,
                BodyPart::Abs,
                BodyPart::Other,
            ]
        );
        assert!(day_body_parts([]).is_empty());
    }

    #[test]
    fn summarize_counts_training_days_and_each_part() {
        let days = vec![
            day((2026, 11, 1), &[BodyPart::Chest, BodyPart::Triceps]),
            day((2026, 11, 3), &[BodyPart::Legs]),
        ];
        let summary = summarize(&days);
        assert_eq!(summary.training_days, 2);
        assert_eq!(
            summary.body_part_days,
            vec![
                count(BodyPart::Chest, 1),
                count(BodyPart::Back, 0),
                count(BodyPart::Shoulders, 0),
                count(BodyPart::BicepsForearm, 0),
                count(BodyPart::Triceps, 1),
                count(BodyPart::Legs, 1),
                count(BodyPart::Abs, 0),
                count(BodyPart::Other, 0),
            ]
        );
        let total: u32 = summary.body_part_days.iter().map(|entry| entry.days).sum();
        assert_eq!(total, 3, "the parts exceed the training days");
    }

    #[test]
    fn summarize_counts_a_repeated_date_once() {
        let days = vec![day((2026, 11, 1), &[BodyPart::Chest])];
        let mut duplicated = days.clone();
        duplicated.extend(days);
        let summary = summarize(&duplicated);
        assert_eq!(summary.training_days, 1);
        assert_eq!(summary.body_part_days[0], count(BodyPart::Chest, 1));
    }

    #[test]
    fn month_view_lists_the_days_in_order_and_keeps_the_counts() {
        let month = JstDate::new(2026, 11, 1).unwrap().month_range();
        let days = vec![
            day((2026, 11, 3), &[BodyPart::Legs]),
            day((2026, 11, 1), &[BodyPart::Chest, BodyPart::Triceps]),
        ];
        let view = month_view(month, days);

        assert_eq!(view.month, "2026-11");
        assert_eq!(view.days.len(), 2);
        assert_eq!(
            view.days,
            vec![
                CalendarDay {
                    date: JstDate::new(2026, 11, 1).unwrap(),
                    body_parts: vec![BodyPart::Chest, BodyPart::Triceps],
                },
                CalendarDay {
                    date: JstDate::new(2026, 11, 3).unwrap(),
                    body_parts: vec![BodyPart::Legs],
                },
            ]
        );
        assert_eq!(view.training_days, 2);
        assert_eq!(view.body_part_days[0], count(BodyPart::Chest, 1));
    }

    #[test]
    fn month_view_of_an_empty_month_has_no_days() {
        let month = JstDate::new(2026, 2, 1).unwrap().month_range();
        let view = month_view(month, Vec::new());
        assert!(view.days.is_empty());
        assert_eq!(view.training_days, 0);
        assert_eq!(view.body_part_days.len(), 8);
        assert!(view.body_part_days.iter().all(|entry| entry.days == 0));
        assert_eq!(view.start, JstDate::new(2026, 2, 1).unwrap());
        assert_eq!(view.end, JstDate::new(2026, 2, 28).unwrap());
    }

    #[test]
    fn the_month_view_round_trips_through_serde() {
        let month = JstDate::new(2026, 11, 1).unwrap().month_range();
        let view = month_view(
            month,
            vec![day((2026, 11, 1), &[BodyPart::Chest, BodyPart::Back])],
        );
        let json = serde_json::to_string(&view).expect("a serializable month");
        assert_eq!(
            serde_json::from_str::<super::CalendarMonth>(&json).expect("a readable month"),
            view
        );
    }

    #[test]
    fn set_detail_uses_the_set_calc_totals() {
        let key = BestKey {
            exercise_id: "bench".to_owned(),
            grip: Some("pronated_medium".to_owned()),
            attachment: None,
        };
        let warmup = SetRecord::new(
            key.clone(),
            SetKind::Warmup,
            vec![segment(60.0, 10)],
            None,
            None,
            RecordMode::WeightAndReps,
        )
        .unwrap();
        let warmup_detail = set_detail(&warmup);
        assert_eq!(warmup_detail.kind, SetKind::Warmup);
        assert_eq!(warmup_detail.total_weight_kg, 0.0);
        assert_eq!(warmup_detail.grip.as_deref(), Some("pronated_medium"));
        assert_eq!(warmup_detail.duration_seconds, None);

        let drop_set = SetRecord::new(
            key,
            SetKind::Drop,
            vec![segment(100.0, 8), segment(80.0, 6)],
            Some(8.0),
            None,
            RecordMode::WeightAndReps,
        )
        .unwrap();
        let detail = set_detail(&drop_set);
        assert_eq!(detail.total_weight_kg, 1_280.0);
        assert_eq!(detail.segments.len(), 2);
        assert_eq!(detail.rpe, Some(8.0));

        let plank = SetRecord::new(
            BestKey {
                exercise_id: "plank".to_owned(),
                grip: None,
                attachment: None,
            },
            SetKind::Normal,
            Vec::new(),
            None,
            Some(90),
            RecordMode::Time,
        )
        .unwrap();
        let detail = set_detail(&plank);
        assert_eq!(detail.total_weight_kg, 0.0);
        assert_eq!(detail.duration_seconds, Some(90));
    }

    #[test]
    fn a_unilateral_set_detail_keeps_the_sides() {
        let record = SetRecord::new(
            BestKey {
                exercise_id: "curl".to_owned(),
                grip: None,
                attachment: None,
            },
            SetKind::Normal,
            vec![SetSegment {
                weight_kg: 0.0,
                reps: 0,
                momentum_reps: 0,
                assisted_reps: 0,
                left: Some(SideValue {
                    weight_kg: 10.0,
                    reps: 5,
                    duration_seconds: None,
                }),
                right: Some(SideValue {
                    weight_kg: 12.0,
                    reps: 4,
                    duration_seconds: None,
                }),
                duration_seconds: None,
            }],
            None,
            None,
            RecordMode::WeightAndReps,
        )
        .unwrap();
        let detail: CalendarSet = set_detail(&record);
        assert_eq!(detail.total_weight_kg, 98.0);
        assert_eq!(
            detail.segments[0].left.as_ref().map(|side| side.reps),
            Some(5)
        );
        assert_eq!(
            detail.segments[0].right.as_ref().map(|side| side.reps),
            Some(4)
        );
    }

    fn day(date: (i64, u32, u32), parts: &[BodyPart]) -> TrainingDay {
        TrainingDay {
            date: JstDate::new(date.0, date.1, date.2).unwrap(),
            body_parts: parts.to_vec(),
        }
    }

    fn count(body_part: BodyPart, days: u32) -> super::BodyPartDayCount {
        super::BodyPartDayCount { body_part, days }
    }

    fn segment(weight_kg: f64, reps: u32) -> SetSegment {
        SetSegment {
            weight_kg,
            reps,
            momentum_reps: 0,
            assisted_reps: 0,
            left: None,
            right: None,
            duration_seconds: None,
        }
    }
}
