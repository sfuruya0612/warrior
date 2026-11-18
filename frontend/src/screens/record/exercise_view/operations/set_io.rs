//! セットの追加、削除、コピーと、端末内への保存。

use dioxus::prelude::*;
use warrior_core::ai::output::SetProposal;
use warrior_core::exercise::{GripSelection, RecordMode};
use warrior_core::set_calc::SetKind;
use warrior_core::set_rules;

use super::super::view::ExerciseView;
use super::{find_view_and_set, SetSaver};
use crate::screens::record::errors::{
    clear_set_error, set_load_error, set_set_error, RecordErrors,
};
use crate::screens::record::sets::{default_grip, is_lateral, previous_values, SetView};
use crate::storage;
use crate::sync;
use crate::text::Key;
use crate::training_set::{self, SegmentSideValues, SetSegmentValues, SetValues};

/// セットの追加を画面へ反映し、端末内へ保存する。
pub(super) async fn add_set(
    training_id: String,
    view_id: String,
    views: Signal<Vec<ExerciseView>>,
    error: Signal<RecordErrors>,
) {
    add_set_with(training_id, view_id, views, error, default_values).await;
}

/// AI の提案のセットを、提案のセット数だけ追加する。
pub(super) async fn apply_proposal(
    training_id: String,
    view_id: String,
    proposal: SetProposal,
    views: Signal<Vec<ExerciseView>>,
    error: Signal<RecordErrors>,
) {
    for _ in 0..proposal.sets {
        let current = proposal.clone();
        if !add_set_with(
            training_id.clone(),
            view_id.clone(),
            views,
            error,
            move |view| proposal_values(view, &current),
        )
        .await
        {
            return;
        }
    }
}

async fn add_set_with(
    training_id: String,
    view_id: String,
    mut views: Signal<Vec<ExerciseView>>,
    error: Signal<RecordErrors>,
    values_for: impl Fn(&ExerciseView) -> SetValues,
) -> bool {
    let Some(view) = (views)()
        .iter()
        .find(|view| view.record.id == view_id)
        .cloned()
    else {
        return false;
    };
    let position = view
        .sets
        .iter()
        .map(|set| set.record.position)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let values = values_for(&view);
    let (rows, operation) = match training_set::create(&training_id, &view_id, position, &values) {
        Ok(rows) => rows,
        Err(_) => {
            set_load_error(error, Key::RecordSetSaveFailed);
            return false;
        }
    };
    let set_view = SetView::from_rows(rows.set.clone(), rows.segments.clone());
    {
        let mut list = (views)();
        if let Some(view) = list.iter_mut().find(|view| view.record.id == view_id) {
            view.sets.push(set_view);
        }
        views.set(list);
    }
    let result = match storage::current().await {
        Ok(storage) => {
            storage
                .save_set_create(&rows.set, &rows.segments, &operation)
                .await
        }
        Err(error) => Err(error),
    };
    if result.is_err() {
        let mut list = (views)();
        if let Some(view) = list.iter_mut().find(|view| view.record.id == view_id) {
            view.sets.retain(|set| set.record.id != rows.set.id);
        }
        views.set(list);
        set_load_error(error, Key::RecordSetSaveFailed);
        return false;
    }
    sync::flush_in_background();
    if let Some(grip) = values.grip {
        super::grip::refresh_after_add(&training_id, &view_id, &rows.set.id, &view, grip, views)
            .await;
    }
    true
}

/// 空のセットの値。
fn default_values(view: &ExerciseView) -> SetValues {
    let lateral = is_lateral(view.snapshot.laterality, view.snapshot.record_mode);
    let side = lateral.then_some(SegmentSideValues {
        weight_kg: match view.snapshot.record_mode {
            RecordMode::BodyweightAndReps => Some(0.0),
            RecordMode::WeightAndReps | RecordMode::Time => None,
        },
        reps: 0,
        duration_seconds: None,
    });
    SetValues {
        kind: SetKind::Normal,
        mode: view.snapshot.record_mode,
        segments: vec![SetSegmentValues {
            weight_kg: 0.0,
            reps: 0,
            momentum_reps: 0,
            assisted_reps: 0,
            duration_seconds: None,
            left: side,
            right: side,
        }],
        completed: false,
        body_weight_kg: view.body_weight_kg,
        rpe: None,
        grip: default_grip(
            view.snapshot.grip_mode,
            &view.snapshot.grip_options,
            view.sets.last().and_then(SetView::grip),
        ),
    }
}

/// AI の提案の値を持つセットの値。
fn proposal_values(view: &ExerciseView, proposal: &SetProposal) -> SetValues {
    let lateral = is_lateral(view.snapshot.laterality, view.snapshot.record_mode);
    let weight = proposal.weight_kg.unwrap_or(0.0);
    let reps = proposal.reps.unwrap_or(0);
    let side = lateral.then_some(SegmentSideValues {
        weight_kg: match view.snapshot.record_mode {
            RecordMode::BodyweightAndReps => Some(0.0),
            RecordMode::WeightAndReps | RecordMode::Time => None,
        },
        reps,
        duration_seconds: None,
    });
    SetValues {
        kind: SetKind::Normal,
        mode: view.snapshot.record_mode,
        segments: vec![SetSegmentValues {
            weight_kg: weight,
            reps,
            momentum_reps: 0,
            assisted_reps: 0,
            duration_seconds: proposal.duration_seconds,
            left: side,
            right: side,
        }],
        completed: false,
        body_weight_kg: view.body_weight_kg,
        rpe: None,
        grip: proposal
            .grip
            .as_deref()
            .and_then(|grip| serde_json::from_str::<GripSelection>(grip).ok())
            .or_else(|| {
                default_grip(
                    view.snapshot.grip_mode,
                    &view.snapshot.grip_options,
                    view.sets.last().and_then(SetView::grip),
                )
            }),
    }
}

/// 入力の最新の値を端末内へ保存する。保存中に新しい入力があれば繰り返す。
/// 戻り値は、この呼び出しで最新の値を保存できたかである。
pub(super) async fn save_set_loop(
    training_id: String,
    set_id: String,
    views: Signal<Vec<ExerciseView>>,
    error: Signal<RecordErrors>,
    saver: SetSaver,
) -> bool {
    if !saver.start(&set_id) {
        return false;
    }
    let Some((mode, lateral)) = settings_for_set(views, &set_id) else {
        saver.finish(&set_id);
        return false;
    };
    while let Some(set) = find_set(views, &set_id) {
        if saver.is_stopped(&set_id) || !completed_can_save(&saver, &set) {
            break;
        }
        if let Some(key) = set.validation_error(lateral) {
            set_set_error(error, &set_id, key);
            break;
        }
        let revision = set.revision;
        let values = set.values(lateral, mode);
        let segment_records = set
            .segments
            .iter()
            .map(|segment| segment.record.clone())
            .collect::<Vec<_>>();
        let (rows, operation) =
            match training_set::update(&training_id, &set.record, &segment_records, &values) {
                Ok(rows) => rows,
                Err(_) => {
                    set_set_error(error, &set_id, Key::RecordSetSaveFailed);
                    break;
                }
            };
        let result = match storage::current().await {
            Ok(storage) => {
                if saving_stopped(views, &saver, &set_id) {
                    saver.finish(&set_id);
                    return false;
                }
                storage
                    .save_set_update(&rows.set, &rows.segments, &operation)
                    .await
            }
            Err(error) => Err(error),
        };
        if result.is_err() {
            set_set_error(error, &set_id, Key::RecordSetSaveFailed);
            break;
        }
        clear_set_error(error, &set_id);
        sync::flush_in_background();
        if saving_stopped(views, &saver, &set_id) {
            saver.finish(&set_id);
            return true;
        }
        match find_set(views, &set_id) {
            Some(latest) if latest.revision == revision => {
                // 版を確かめてから保存の印を外すまでの間にも入力が届く。
                // 印を外した後にもう一度確かめ、変わっていれば自分で続ける。
                saver.finish(&set_id);
                if saving_stopped(views, &saver, &set_id) {
                    return true;
                }
                match find_set(views, &set_id) {
                    Some(latest) if latest.revision == revision => return true,
                    _ => {
                        if !saver.start(&set_id) {
                            return false;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    saver.finish(&set_id);
    false
}

/// 前回の値をセットの行としてコピーする。
pub(super) async fn copy_sets(
    training_id: String,
    view_id: String,
    mut views: Signal<Vec<ExerciseView>>,
    error: Signal<RecordErrors>,
) {
    let Some(view) = (views)()
        .iter()
        .find(|view| view.record.id == view_id)
        .cloned()
    else {
        return;
    };
    let Some(previous) = view.previous.as_ref() else {
        return;
    };
    let values = previous
        .sets
        .iter()
        .filter_map(|set| {
            previous_values(
                set,
                view.snapshot.record_mode,
                view.body_weight_kg,
                view.snapshot.laterality,
            )
        })
        .collect::<Vec<_>>();
    if values.is_empty() {
        return;
    }
    let storage = match storage::current().await {
        Ok(storage) => storage,
        Err(_) => {
            set_load_error(error, Key::RecordSetSaveFailed);
            return;
        }
    };
    // 末尾から削除する。削除するセットの直後は常に空であり、直後のドロップセットが
    // あるセットの削除を避けられる。
    for set in view.sets.iter().rev() {
        let operation = match training_set::delete(&training_id, &set.record.id) {
            Ok(operation) => operation,
            Err(_) => {
                set_load_error(error, Key::RecordSetSaveFailed);
                return;
            }
        };
        if storage
            .save_set_delete(&training_id, &set.record.id, &operation)
            .await
            .is_err()
        {
            set_load_error(error, Key::RecordSetSaveFailed);
            restore_sets(views, &view_id, error).await;
            return;
        }
    }
    let mut copied = Vec::new();
    for (index, values) in values.iter().enumerate() {
        let position = i64::try_from(index).unwrap_or(i64::MAX).saturating_add(1);
        let (rows, operation) = match training_set::create(&training_id, &view_id, position, values)
        {
            Ok(rows) => rows,
            Err(_) => {
                set_load_error(error, Key::RecordSetSaveFailed);
                restore_sets(views, &view_id, error).await;
                return;
            }
        };
        if storage
            .save_set_create(&rows.set, &rows.segments, &operation)
            .await
            .is_err()
        {
            set_load_error(error, Key::RecordSetSaveFailed);
            restore_sets(views, &view_id, error).await;
            return;
        }
        copied.push(SetView::from_rows(rows.set, rows.segments));
    }
    {
        let mut list = (views)();
        if let Some(view) = list.iter_mut().find(|view| view.record.id == view_id) {
            view.sets = copied;
        }
        views.set(list);
    }
    sync::flush_in_background();
}

/// セットを画面から消し、端末内の行と送信待ちの列から削除する。
pub(super) async fn delete_set(
    training_id: String,
    set_id: String,
    mut views: Signal<Vec<ExerciseView>>,
    error: Signal<RecordErrors>,
) {
    let Some((view, _set)) = find_view_and_set(views, &set_id) else {
        return;
    };
    let next = view
        .sets
        .iter()
        .position(|set| set.record.id == set_id)
        .and_then(|index| view.sets.get(index + 1))
        .map(SetView::kind);
    if !set_rules::delete_set_allowed(next) {
        set_set_error(error, &set_id, Key::RecordSetDeleteBlockedByDrop);
        return;
    }
    let operation = match training_set::delete(&training_id, &set_id) {
        Ok(operation) => operation,
        Err(_) => {
            set_set_error(error, &set_id, Key::RecordSetSaveFailed);
            return;
        }
    };
    {
        let mut list = (views)();
        for view in list.iter_mut() {
            view.sets.retain(|set| set.record.id != set_id);
        }
        views.set(list);
    }
    let result = match storage::current().await {
        Ok(storage) => {
            storage
                .save_set_delete(&training_id, &set_id, &operation)
                .await
        }
        Err(error) => Err(error),
    };
    if result.is_err() {
        set_set_error(error, &set_id, Key::RecordSetSaveFailed);
        restore_sets(views, &view.record.id, error).await;
        return;
    }
    clear_set_error(error, &set_id);
    sync::flush_in_background();
}

/// 保存済みのセットを画面へ戻す。
pub(super) async fn restore_sets(
    mut views: Signal<Vec<ExerciseView>>,
    view_id: &str,
    error: Signal<RecordErrors>,
) {
    let Ok(storage) = storage::current().await else {
        set_load_error(error, Key::RecordSetSaveFailed);
        return;
    };
    let Ok(stored) = storage.training_sets(view_id).await else {
        set_load_error(error, Key::RecordSetSaveFailed);
        return;
    };
    let sets = stored
        .into_iter()
        .filter_map(SetView::from_stored)
        .collect::<Vec<_>>();
    let mut list = (views)();
    if let Some(view) = list.iter_mut().find(|view| view.record.id == view_id) {
        view.sets = sets;
    }
    views.set(list);
}

fn find_set(views: Signal<Vec<ExerciseView>>, set_id: &str) -> Option<SetView> {
    (views)()
        .iter()
        .flat_map(|view| view.sets.iter())
        .find(|set| set.record.id == set_id)
        .cloned()
}

/// 完了済みのセットを保存してよいか。終了したトレーニングのセットだけ編集できる。
pub(super) fn completed_can_save(saver: &SetSaver, set: &SetView) -> bool {
    !set.completed() || saver.editable_completed()
}

/// 保存を続けない状態か。止めたセット、保存しない完了済みのセット、画面から消えたセット。
fn saving_stopped(views: Signal<Vec<ExerciseView>>, saver: &SetSaver, set_id: &str) -> bool {
    saver.is_stopped(set_id)
        || find_set(views, set_id).is_none_or(|set| !completed_can_save(saver, &set))
}

fn settings_for_set(views: Signal<Vec<ExerciseView>>, set_id: &str) -> Option<(RecordMode, bool)> {
    (views)()
        .iter()
        .find(|view| view.sets.iter().any(|set| set.record.id == set_id))
        .map(|view| {
            let mode = view.snapshot.record_mode;
            (mode, is_lateral(view.snapshot.laterality, mode))
        })
}
