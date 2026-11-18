//! カレンダー画面の表示の組み立て。
//!
//! 月のセルの並び、部位の色と名前、セットの文言を、画面に依存しない関数として持つ。

use warrior_core::calendar::{day_body_parts, month_cells, CalendarMonth};
use warrior_core::exercise::BodyPart;
use warrior_core::jst::{JstDate, JstMonth, Weekday};
use warrior_core::rpe;
use warrior_core::set_calc::{SetKind, SetSegment, SideValue};

use crate::grip_text::{attachment_label, grip_label};
use crate::text::{text, Key};

/// カレンダーの 1 日のセルの表示内容。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CalendarCell {
    pub date: JstDate,
    pub body_parts: Vec<BodyPart>,
    pub selected: bool,
    pub today: bool,
    pub future: bool,
}

/// 月表示のセルの並びを作る。月の外のセルは None にする。
pub(crate) fn build_cells(
    month: Option<JstMonth>,
    view: Option<&CalendarMonth>,
    selected: Option<JstDate>,
    today: Option<JstDate>,
) -> Vec<Option<CalendarCell>> {
    let Some(month) = month else {
        return Vec::new();
    };
    let days = view.map(|view| view.days.as_slice()).unwrap_or_default();
    month_cells(month)
        .into_iter()
        .map(|cell| {
            cell.map(|date| {
                let day = days.iter().find(|day| day.date == date);
                CalendarCell {
                    date,
                    body_parts: day.map(|day| day.body_parts.clone()).unwrap_or_default(),
                    selected: selected == Some(date),
                    today: today == Some(date),
                    future: today.is_some_and(|today| date > today),
                }
            })
        })
        .collect()
}

/// 8 区分すべての日数。読み込み前は 0 にする。
pub(crate) fn part_counts(view: Option<&CalendarMonth>) -> Vec<(BodyPart, u32)> {
    BodyPart::ALL
        .into_iter()
        .map(|part| {
            let days = view
                .and_then(|view| {
                    view.body_part_days
                        .iter()
                        .find(|entry| entry.body_part == part)
                        .map(|entry| entry.days)
                })
                .unwrap_or(0);
            (part, days)
        })
        .collect()
}

pub(crate) fn day_cell_class(cell: &CalendarCell) -> String {
    let mut class = String::from(
        "flex h-[50px] flex-col items-center gap-1.5 border-t border-line pt-1.5 \
         font-condensed text-base font-semibold",
    );
    if cell.selected {
        class.push_str(" bg-surface-raised");
    }
    class
}

pub(crate) fn day_number_class(cell: &CalendarCell) -> &'static str {
    if cell.today {
        "flex h-6 w-[26px] items-center justify-center border-[1.5px] border-primary text-primary"
    } else if cell.future {
        "flex h-6 items-center justify-center text-text-sub"
    } else {
        "flex h-6 items-center justify-center"
    }
}

/// 曜日の見出し。月曜始まり。
pub(crate) fn weekdays() -> Vec<&'static str> {
    [
        Weekday::Monday,
        Weekday::Tuesday,
        Weekday::Wednesday,
        Weekday::Thursday,
        Weekday::Friday,
        Weekday::Saturday,
        Weekday::Sunday,
    ]
    .into_iter()
    .map(weekday_label)
    .collect()
}

fn weekday_label(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Monday => text(Key::CalendarWeekdayMonday),
        Weekday::Tuesday => text(Key::CalendarWeekdayTuesday),
        Weekday::Wednesday => text(Key::CalendarWeekdayWednesday),
        Weekday::Thursday => text(Key::CalendarWeekdayThursday),
        Weekday::Friday => text(Key::CalendarWeekdayFriday),
        Weekday::Saturday => text(Key::CalendarWeekdaySaturday),
        Weekday::Sunday => text(Key::CalendarWeekdaySunday),
    }
}

pub(crate) fn part_label(part: BodyPart) -> &'static str {
    match part {
        BodyPart::Chest => text(Key::CalendarPartChest),
        BodyPart::Back => text(Key::CalendarPartBack),
        BodyPart::Shoulders => text(Key::CalendarPartShoulders),
        BodyPart::BicepsForearm => text(Key::CalendarPartBicepsForearm),
        BodyPart::Triceps => text(Key::CalendarPartTriceps),
        BodyPart::Legs => text(Key::CalendarPartLegs),
        BodyPart::Abs => text(Key::CalendarPartAbs),
        BodyPart::Other => text(Key::CalendarPartOther),
    }
}

pub(crate) fn part_color_class(part: BodyPart) -> &'static str {
    match part {
        BodyPart::Chest => "bg-part-chest",
        BodyPart::Back => "bg-part-back",
        BodyPart::Shoulders => "bg-part-shoulders",
        BodyPart::BicepsForearm => "bg-part-biceps-forearm",
        BodyPart::Triceps => "bg-part-triceps",
        BodyPart::Legs => "bg-part-legs",
        BodyPart::Abs => "bg-part-abs",
        BodyPart::Other => "bg-part-other",
    }
}

pub(crate) fn day_aria_label(cell: &CalendarCell) -> String {
    let title = day_title(cell.date);
    if cell.body_parts.is_empty() {
        return title;
    }
    let parts = cell
        .body_parts
        .iter()
        .map(|part| part_label(*part))
        .collect::<Vec<_>>()
        .join("、");
    format!("{title}: {parts}")
}

pub(crate) fn day_title(date: JstDate) -> String {
    format!(
        "{}月{}日({})",
        date.month(),
        date.day(),
        weekday_label(date.weekday())
    )
}

pub(crate) fn day_parts(trainings: &[warrior_core::calendar::CalendarTraining]) -> Vec<BodyPart> {
    day_body_parts(
        trainings
            .iter()
            .flat_map(|training| training.exercises.iter().map(|exercise| exercise.body_part)),
    )
}

pub(crate) fn set_text(set: &warrior_core::calendar::CalendarSet) -> String {
    if let Some(seconds) = set.duration_seconds {
        return format_seconds(seconds);
    }
    set.segments
        .iter()
        .map(segment_text)
        .collect::<Vec<_>>()
        .join(" → ")
}

fn segment_text(segment: &SetSegment) -> String {
    match (&segment.left, &segment.right) {
        (Some(left), Some(right)) => format!(
            "{} {} / {} {}",
            text(Key::CalendarLeft),
            side_text(left),
            text(Key::CalendarRight),
            side_text(right)
        ),
        _ => format!(
            "{} kg × {} {}",
            segment.weight_kg,
            segment.reps,
            text(Key::CalendarReps)
        ),
    }
}

fn side_text(side: &SideValue) -> String {
    format!(
        "{} kg × {} {}",
        side.weight_kg,
        side.reps,
        text(Key::CalendarReps)
    )
}

pub(crate) fn set_notes(set: &warrior_core::calendar::CalendarSet) -> Vec<String> {
    let mut notes = vec![set_kind_label(set.kind).to_owned()];
    if let Some(rpe) = set.rpe {
        notes.push(format!("RPE {}", rpe::format(rpe)));
    }
    if let Some(grip) = set.grip.as_deref() {
        notes.push(grip_label(grip));
    }
    if let Some(attachment) = set.attachment.as_deref() {
        notes.push(attachment_label(attachment));
    }
    for segment in &set.segments {
        if segment.momentum_reps > 0 {
            push_note(
                &mut notes,
                format!(
                    "{} {} {}",
                    text(Key::CalendarMomentum),
                    segment.momentum_reps,
                    text(Key::CalendarReps)
                ),
            );
        }
        if segment.assisted_reps > 0 {
            push_note(
                &mut notes,
                format!(
                    "{} {} {}",
                    text(Key::CalendarAssisted),
                    segment.assisted_reps,
                    text(Key::CalendarReps)
                ),
            );
        }
    }
    notes
}

fn push_note(notes: &mut Vec<String>, note: String) {
    if !notes.contains(&note) {
        notes.push(note);
    }
}

fn set_kind_label(kind: SetKind) -> &'static str {
    match kind {
        SetKind::Normal => text(Key::CalendarSetKindNormal),
        SetKind::Warmup => text(Key::CalendarSetKindWarmup),
        SetKind::Drop => text(Key::CalendarSetKindDrop),
    }
}

pub(crate) fn format_weight(value: f64) -> String {
    format!("{value} kg")
}

pub(crate) fn format_seconds(seconds: u32) -> String {
    let minutes = seconds / 60;
    let remaining = seconds % 60;
    if minutes > 0 {
        format!(
            "{minutes}{}{remaining}{}",
            text(Key::CalendarMinutes),
            text(Key::CalendarSeconds)
        )
    } else {
        format!("{remaining}{}", text(Key::CalendarSeconds))
    }
}

#[cfg(test)]
mod tests {
    //! カレンダーの表示の組み立ての単体テスト。
    //!
    //! 要求の受入基準は E2E が担い、ここは文言と並びの内部条件を確かめる。

    use warrior_core::calendar::{
        CalendarDayDetail, CalendarExercise, CalendarMonth, CalendarSet, CalendarTraining,
        TrainingDay,
    };
    use warrior_core::exercise::{BodyPart, Laterality};
    use warrior_core::jst::JstDate;
    use warrior_core::set_calc::{RecordMode, SetKind, SetSegment, SideValue};
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::{
        build_cells, day_aria_label, day_number_class, day_parts, day_title, format_seconds,
        part_color_class, part_counts, part_label, set_notes, set_text, weekdays, CalendarCell,
    };
    use crate::generated::tokens;
    use crate::grip_text::{attachment_label, grip_label};

    #[wasm_bindgen_test]
    fn the_cells_fill_the_weeks_and_mark_the_selected_day() {
        let month = JstDate::new(2026, 11, 12).unwrap().month_range();
        let view = month_view();
        let cells = build_cells(
            Some(month),
            Some(&view),
            Some(JstDate::new(2026, 11, 3).unwrap()),
            Some(JstDate::new(2026, 11, 1).unwrap()),
        );
        assert_eq!(cells.len(), 42);
        assert!(cells[..6].iter().all(Option::is_none));
        let first = cells[6].clone().expect("the first day");
        assert_eq!(first.date, JstDate::new(2026, 11, 1).unwrap());
        assert!(first.today);
        assert_eq!(first.body_parts, vec![BodyPart::Chest, BodyPart::Triceps]);
        let third = cells[8].clone().expect("the third day");
        assert!(third.selected);
        assert_eq!(third.body_parts, vec![BodyPart::Legs]);
        let second = cells[7].clone().expect("the second day");
        assert!(second.body_parts.is_empty());
        assert!(!second.today);
        assert!(!second.selected);
    }

    #[wasm_bindgen_test]
    fn the_part_counts_count_the_days_of_every_part() {
        let counts = part_counts(Some(&month_view()));
        assert_eq!(
            counts,
            vec![
                (BodyPart::Chest, 1),
                (BodyPart::Back, 0),
                (BodyPart::Shoulders, 0),
                (BodyPart::BicepsForearm, 0),
                (BodyPart::Triceps, 1),
                (BodyPart::Legs, 1),
                (BodyPart::Abs, 0),
                (BodyPart::Other, 0),
            ]
        );
        let empty = part_counts(None);
        assert!(
            empty.iter().all(|(_, days)| *days == 0),
            "a month without training has no day on any part"
        );
    }

    #[wasm_bindgen_test]
    fn every_part_has_a_label_and_a_color_class() {
        let labels = BodyPart::ALL.map(part_label);
        let mut unique = labels.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), 8);
        let colors = BodyPart::ALL.map(part_color_class);
        let mut unique = colors.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), 8);
        assert_eq!(part_color_class(BodyPart::Chest), "bg-part-chest");
        assert_eq!(
            part_color_class(BodyPart::BicepsForearm),
            "bg-part-biceps-forearm"
        );
    }

    #[wasm_bindgen_test]
    fn the_light_and_dark_part_colors_are_distinguishable() {
        for colors in [
            [
                tokens::part_color::dark::CHEST,
                tokens::part_color::dark::BACK,
                tokens::part_color::dark::SHOULDERS,
                tokens::part_color::dark::BICEPS_FOREARM,
                tokens::part_color::dark::TRICEPS,
                tokens::part_color::dark::LEGS,
                tokens::part_color::dark::ABS,
                tokens::part_color::dark::OTHER,
            ],
            [
                tokens::part_color::light::CHEST,
                tokens::part_color::light::BACK,
                tokens::part_color::light::SHOULDERS,
                tokens::part_color::light::BICEPS_FOREARM,
                tokens::part_color::light::TRICEPS,
                tokens::part_color::light::LEGS,
                tokens::part_color::light::ABS,
                tokens::part_color::light::OTHER,
            ],
        ] {
            for (index, left) in colors.iter().enumerate() {
                for right in colors.iter().skip(index + 1) {
                    assert_ne!(left, right);
                }
            }
        }
    }

    #[wasm_bindgen_test]
    fn the_weekday_header_starts_on_monday() {
        assert_eq!(weekdays(), vec!["月", "火", "水", "木", "金", "土", "日"]);
    }

    #[wasm_bindgen_test]
    fn the_day_title_has_the_month_day_and_weekday() {
        assert_eq!(
            day_title(JstDate::new(2026, 11, 10).unwrap()),
            "11月10日(火)"
        );
        let cell = CalendarCell {
            date: JstDate::new(2026, 11, 1).unwrap(),
            body_parts: vec![BodyPart::Back, BodyPart::BicepsForearm],
            selected: false,
            today: false,
            future: false,
        };
        assert_eq!(day_aria_label(&cell), "11月1日(日): 背中、二頭・前腕");
        let empty = CalendarCell {
            body_parts: Vec::new(),
            ..cell
        };
        assert_eq!(day_aria_label(&empty), "11月1日(日)");
    }

    #[wasm_bindgen_test]
    fn the_days_after_today_use_the_subtle_text_color() {
        let month = JstDate::new(2026, 11, 12).unwrap().month_range();
        let cells = build_cells(
            Some(month),
            None,
            None,
            Some(JstDate::new(2026, 11, 10).unwrap()),
        );
        let past = cells[14].as_ref().expect("the ninth day");
        let today_cell = cells[15].as_ref().expect("the tenth day");
        let future_cell = cells[16].as_ref().expect("the eleventh day");
        assert_eq!(past.date, JstDate::new(2026, 11, 9).unwrap());
        assert!(today_cell.today);
        assert_eq!(future_cell.date, JstDate::new(2026, 11, 11).unwrap());
        assert!(!past.future);
        assert!(!today_cell.future);
        assert!(future_cell.future);
        assert!(!day_number_class(past).contains("text-text-sub"));
        assert!(day_number_class(today_cell).contains("text-primary"));
        assert!(day_number_class(future_cell).contains("text-text-sub"));
        let without_today = build_cells(Some(month), None, None, None);
        assert!(
            without_today.iter().flatten().all(|cell| !cell.future),
            "a calendar without today has no future day"
        );
    }

    #[wasm_bindgen_test]
    fn the_day_parts_are_unique_and_ordered() {
        let detail = day_detail();
        assert_eq!(
            day_parts(&detail.trainings),
            vec![BodyPart::Chest, BodyPart::Triceps]
        );
        assert!(day_parts(&[]).is_empty());
    }

    #[wasm_bindgen_test]
    fn the_set_text_shows_the_weight_and_the_reps() {
        assert_eq!(set_text(&weight_set(100.0, 5)), "100 kg × 5 回");
        assert_eq!(
            set_text(&CalendarSet {
                kind: SetKind::Drop,
                rpe: None,
                grip: None,
                attachment: None,
                duration_seconds: None,
                total_weight_kg: 1_280.0,
                segments: vec![segment(100.0, 8), segment(80.0, 6)],
            }),
            "100 kg × 8 回 → 80 kg × 6 回"
        );
        assert_eq!(
            set_text(&CalendarSet {
                kind: SetKind::Normal,
                rpe: None,
                grip: None,
                attachment: None,
                duration_seconds: Some(90),
                total_weight_kg: 0.0,
                segments: Vec::new(),
            }),
            "1分30秒"
        );
        let unilateral = CalendarSet {
            kind: SetKind::Normal,
            rpe: None,
            grip: None,
            attachment: None,
            duration_seconds: None,
            total_weight_kg: 98.0,
            segments: vec![SetSegment {
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
        };
        assert_eq!(set_text(&unilateral), "左 10 kg × 5 回 / 右 12 kg × 4 回");
    }

    #[wasm_bindgen_test]
    fn the_set_notes_show_the_kind_rpe_grip_and_counts() {
        let set = CalendarSet {
            kind: SetKind::Normal,
            rpe: Some(8.0),
            grip: Some("pronated_medium".to_owned()),
            attachment: None,
            duration_seconds: None,
            total_weight_kg: 500.0,
            segments: vec![SetSegment {
                momentum_reps: 2,
                assisted_reps: 1,
                ..segment(100.0, 5)
            }],
        };
        assert_eq!(
            set_notes(&set),
            vec![
                "通常".to_owned(),
                "RPE 8.0".to_owned(),
                "順手 ミディアム".to_owned(),
                "反動 2 回".to_owned(),
                "補助 1 回".to_owned(),
            ]
        );
        assert_eq!(set_notes(&weight_set(100.0, 5)), vec!["通常".to_owned()]);
        let attachment = CalendarSet {
            grip: None,
            attachment: Some("mag_wide_over".to_owned()),
            ..weight_set(100.0, 5)
        };
        assert_eq!(
            set_notes(&attachment),
            vec!["通常".to_owned(), "マグワイドオーバー".to_owned()]
        );
    }

    #[wasm_bindgen_test]
    fn the_grip_labels_use_the_stored_key() {
        assert_eq!(grip_label("pronated_wide"), "順手 ワイド");
        assert_eq!(grip_label("supinated_narrow"), "逆手 ナロー");
        assert_eq!(grip_label("parallel_medium"), "パラレル ミディアム");
        assert_eq!(grip_label("unknown"), "unknown");
        assert_eq!(attachment_label("wide_bar"), "ワイドバー");
        assert_eq!(
            attachment_label("mag_narrow_parallel"),
            "マグナローパラレル"
        );
        assert_eq!(attachment_label("unknown"), "unknown");
    }

    #[wasm_bindgen_test]
    fn the_seconds_format_uses_minutes() {
        assert_eq!(format_seconds(0), "0秒");
        assert_eq!(format_seconds(59), "59秒");
        assert_eq!(format_seconds(60), "1分0秒");
        assert_eq!(format_seconds(90), "1分30秒");
    }

    fn month_view() -> CalendarMonth {
        let month = JstDate::new(2026, 11, 1).unwrap().month_range();
        warrior_core::calendar::month_view(
            month,
            vec![
                TrainingDay {
                    date: JstDate::new(2026, 11, 1).unwrap(),
                    body_parts: vec![BodyPart::Chest, BodyPart::Triceps],
                },
                TrainingDay {
                    date: JstDate::new(2026, 11, 3).unwrap(),
                    body_parts: vec![BodyPart::Legs],
                },
            ],
        )
    }

    fn day_detail() -> CalendarDayDetail {
        CalendarDayDetail {
            date: JstDate::new(2026, 11, 1).unwrap(),
            total_weight_kg: 500.0,
            trainings: vec![CalendarTraining {
                training_id: "training-a".to_owned(),
                started_at: Some("2026-11-01T03:00:00Z".to_owned()),
                memo: String::new(),
                total_weight_kg: 500.0,
                exercises: vec![
                    CalendarExercise {
                        exercise_id: "bench".to_owned(),
                        name: "ベンチプレス".to_owned(),
                        body_part: BodyPart::Chest,
                        record_mode: RecordMode::WeightAndReps,
                        laterality: Laterality::Bilateral,
                        total_weight_kg: 500.0,
                        sets: vec![weight_set(100.0, 5)],
                    },
                    CalendarExercise {
                        exercise_id: "pushdown".to_owned(),
                        name: "プレスダウン".to_owned(),
                        body_part: BodyPart::Triceps,
                        record_mode: RecordMode::WeightAndReps,
                        laterality: Laterality::Bilateral,
                        total_weight_kg: 0.0,
                        sets: Vec::new(),
                    },
                ],
            }],
        }
    }

    fn weight_set(weight_kg: f64, reps: u32) -> CalendarSet {
        CalendarSet {
            kind: SetKind::Normal,
            rpe: None,
            grip: None,
            attachment: None,
            duration_seconds: None,
            total_weight_kg: weight_kg * f64::from(reps),
            segments: vec![segment(weight_kg, reps)],
        }
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
