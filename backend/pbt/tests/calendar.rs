//! カレンダーの日ごとの部位と月の集計の性質テスト (PRD の「カレンダー」)。
//!
//! 対応する要求: `REQ-CLD-001`、`REQ-CLD-002`、`REQ-CLD-003`。
//! トレーニング日数と部位区分ごとの日数が、その月の記録から正しく集計されること、
//! 1 日に複数の部位を行うと部位区分ごとの日数の合計がトレーニング日数を超えることを確かめる。

use proptest::prelude::*;
use warrior_core::calendar::{
    day_body_parts, month_cells, month_days, next_month, parse_month, previous_month, summarize,
    TrainingDay,
};
use warrior_core::exercise::BodyPart;
use warrior_core::jst::{JstDate, JstMonth};

fn month_start() -> impl Strategy<Value = JstDate> {
    (1900i64..=2100, 1u32..=12)
        .prop_map(|(year, month)| JstDate::new(year, month, 1).expect("a first day of a month"))
}

fn body_part_mask() -> impl Strategy<Value = Vec<bool>> {
    prop::collection::vec(any::<bool>(), BodyPart::ALL.len())
}

fn masked_parts(mask: &[bool]) -> Vec<BodyPart> {
    BodyPart::ALL
        .iter()
        .zip(mask)
        .filter(|(_, selected)| **selected)
        .map(|(part, _)| *part)
        .collect()
}

/// 月の中の重複しない日付と、その日に行った部位区分。
fn training_days() -> impl Strategy<Value = (JstMonth, Vec<TrainingDay>)> {
    month_start().prop_flat_map(|start| {
        let month = start.month_range();
        let last_day = month.end().day();
        (
            Just(month),
            prop::collection::vec((0u32..u32::from(last_day), body_part_mask()), 0..=8),
        )
            .prop_map(move |(month, entries)| {
                let days = entries
                    .into_iter()
                    .map(|(offset, mask)| TrainingDay {
                        date: start
                            .add_days(i64::from(offset))
                            .expect("a day of the month"),
                        body_parts: masked_parts(&mask),
                    })
                    .collect();
                (month, days)
            })
    })
}

/// 月の中の重複しない日付と、1 日 1 部位のトレーニング。
fn single_part_days() -> impl Strategy<Value = (JstMonth, Vec<TrainingDay>)> {
    month_start().prop_flat_map(|start| {
        let month = start.month_range();
        let last_day = month.end().day();
        (
            Just(month),
            prop::collection::vec(
                (0u32..u32::from(last_day), 0usize..BodyPart::ALL.len()),
                0..=8,
            ),
        )
            .prop_map(move |(month, entries)| {
                let mut dates = entries
                    .into_iter()
                    .map(|(offset, index)| {
                        (
                            start
                                .add_days(i64::from(offset))
                                .expect("a day of the month"),
                            BodyPart::ALL[index],
                        )
                    })
                    .collect::<Vec<_>>();
                dates.sort_unstable_by_key(|(date, _)| *date);
                dates.dedup_by_key(|(date, _)| *date);
                let days = dates
                    .into_iter()
                    .map(|(date, part)| TrainingDay {
                        date,
                        body_parts: vec![part],
                    })
                    .collect();
                (month, days)
            })
    })
}

/// 月の中の重複しない日付と、2 区分以上の部位を行った日を少なくとも 1 日含むトレーニング。
fn multi_part_days() -> impl Strategy<Value = (JstMonth, Vec<TrainingDay>)> {
    month_start().prop_flat_map(|start| {
        let month = start.month_range();
        let last_day = month.end().day();
        (
            Just(month),
            prop::collection::vec(0u32..u32::from(last_day), 1..=8),
            body_part_mask(),
            prop::collection::vec(body_part_mask(), 7),
        )
            .prop_map(move |(month, offsets, first_mask, other_masks)| {
                let mut dates = offsets;
                dates.sort_unstable();
                dates.dedup();
                let days = dates
                    .into_iter()
                    .enumerate()
                    .map(|(index, offset)| {
                        let parts = if index == 0 {
                            let mut parts = masked_parts(&first_mask);
                            for part in [BodyPart::Chest, BodyPart::Back] {
                                if !parts.contains(&part) {
                                    parts.push(part);
                                }
                            }
                            day_body_parts(parts)
                        } else {
                            masked_parts(&other_masks[index - 1])
                        };
                        TrainingDay {
                            date: start
                                .add_days(i64::from(offset))
                                .expect("a day of the month"),
                            body_parts: parts,
                        }
                    })
                    .collect();
                (month, days)
            })
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn summarize_counts_distinct_days_and_each_part((_month, days) in training_days()) {
        let summary = summarize(&days);
        let mut dates = days.iter().map(|day| day.date).collect::<Vec<_>>();
        dates.sort_unstable();
        dates.dedup();
        prop_assert_eq!(summary.training_days, dates.len() as u32);
        prop_assert_eq!(summary.body_part_days.len(), BodyPart::ALL.len());
        for (entry, part) in summary.body_part_days.iter().zip(BodyPart::ALL) {
            prop_assert_eq!(entry.body_part, part);
            let expected = dates
                .iter()
                .filter(|date| {
                    days.iter().any(|day| day.date == **date && day.body_parts.contains(&part))
                })
                .count() as u32;
            prop_assert_eq!(entry.days, expected);
        }
    }

    #[test]
    fn one_part_a_day_makes_the_part_days_equal_the_training_days(
        (_month, days) in single_part_days(),
    ) {
        let summary = summarize(&days);
        let total: u32 = summary.body_part_days.iter().map(|entry| entry.days).sum();
        let distinct = {
            let mut dates = days.iter().map(|day| day.date).collect::<Vec<_>>();
            dates.sort_unstable();
            dates.dedup();
            dates.len() as u32
        };
        prop_assert_eq!(summary.training_days, distinct);
        prop_assert_eq!(total, summary.training_days);
    }

    #[test]
    fn a_day_with_multiple_parts_makes_the_part_days_exceed_the_training_days(
        (_month, days) in multi_part_days(),
    ) {
        let summary = summarize(&days);
        prop_assert!(
            days.iter().any(|day| day.body_parts.len() >= 2),
            "the input keeps a day with multiple parts"
        );
        let total: u32 = summary.body_part_days.iter().map(|entry| entry.days).sum();
        prop_assert!(
            total > summary.training_days,
            "the part days {total} must exceed the training days {}",
            summary.training_days
        );
    }

    #[test]
    fn month_cells_align_the_month_to_monday_weeks(start in month_start()) {
        let month = start.month_range();
        let cells = month_cells(month);
        prop_assert_eq!(cells.len() % 7, 0);
        let leading = cells.iter().take_while(|cell| cell.is_none()).count();
        prop_assert_eq!(leading, usize::from(month.start().weekday().index()));
        let days = month_days(month);
        let dates = cells.iter().flatten().copied().collect::<Vec<_>>();
        prop_assert_eq!(dates, days.clone());
        let trailing = cells.iter().rev().take_while(|cell| cell.is_none()).count();
        prop_assert_eq!(trailing, cells.len() - leading - days.len());
    }

    #[test]
    fn day_body_parts_keep_the_eight_part_order(mask in body_part_mask()) {
        let expected = masked_parts(&mask);
        let mut rotated = expected.clone();
        if !rotated.is_empty() {
            rotated.rotate_left(1);
        }
        prop_assert_eq!(day_body_parts(rotated), expected.clone());
        let reversed = expected.iter().rev().copied().collect::<Vec<_>>();
        prop_assert_eq!(day_body_parts(reversed), expected.clone());
        let mut unique = expected.clone();
        unique.dedup();
        prop_assert_eq!(unique.len(), expected.len());
        let positions = expected
            .iter()
            .filter_map(|part| BodyPart::ALL.iter().position(|candidate| candidate == part))
            .collect::<Vec<_>>();
        prop_assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn the_month_text_round_trips(start in month_start()) {
        let month = start.month_range();
        let text = format!("{:04}-{:02}", month.start().year(), month.start().month());
        prop_assert_eq!(parse_month(&text), Some(month));
    }

    #[test]
    fn adjacent_months_round_trip(start in month_start()) {
        let month = start.month_range();
        let next = next_month(month).expect("the next month");
        prop_assert_eq!(previous_month(next), Some(month));
        let previous = previous_month(month).expect("the previous month");
        prop_assert_eq!(next_month(previous), Some(month));
        prop_assert_eq!(
            month.end().add_days(1).map(|date| date.month_range()),
            Some(next)
        );
    }
}
