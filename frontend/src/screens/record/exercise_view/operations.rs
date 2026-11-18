//! セットの入力、追加、削除、並べ替え、完了と、端末内への保存。

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::future::poll_fn;
use std::rc::Rc;
use std::task::{Poll, Waker};

use dioxus::prelude::*;
use warrior_core::exercise::RecordMode;
use warrior_core::rpe;
use warrior_core::set_calc::SetKind;
use warrior_core::superset::rest_timer_starts;
use warrior_core::sync::Operation;
use warrior_core::training_record::SetRecordRow;

use super::super::errors::{clear_set_error, set_set_error, RecordErrors};
use super::super::set_events::{SetCompleted, SetCompletedEvents};
use super::super::sets::{
    is_lateral, kind_allowed, move_allowed, Field, SegmentView, SetAction, SetView,
};
use super::view::ExerciseView;
use crate::ids::{new_uuid, now_iso};
use crate::storage::{self, StopwatchSide};
use crate::sync;
use crate::text::Key;
use crate::training_set::{self, SegmentSideValues, SetValues};

mod grip;
mod set_io;

/// セットごとの保存の状態。
#[derive(Default)]
struct SaveStates {
    /// 保存のループが走っているセット。
    in_flight: HashSet<String>,
    /// 完了の書き込みのために、保存のループを止めたセット。
    stopped: HashSet<String>,
    /// 保存の終わりを待つ処理。
    waiters: HashMap<String, Vec<Waker>>,
}

/// セットごとの保存の実行と、完了のための停止を管理する印。
#[derive(Clone, Default)]
pub(in crate::screens::record) struct SetSaver {
    states: Rc<RefCell<SaveStates>>,
    /// 終了したトレーニングで、完了済みのセットを編集できるか。
    editable_completed: Rc<Cell<bool>>,
}

impl SetSaver {
    /// 完了済みのセットを保存してよいかを設定する。
    pub(in crate::screens::record) fn set_editable_completed(&self, editable: bool) {
        self.editable_completed.set(editable);
    }

    /// 完了済みのセットを保存してよいか。
    fn editable_completed(&self) -> bool {
        self.editable_completed.get()
    }

    fn start(&self, set_id: &str) -> bool {
        let mut states = self.states.borrow_mut();
        if states.stopped.contains(set_id) {
            return false;
        }
        states.in_flight.insert(set_id.to_owned())
    }

    fn finish(&self, set_id: &str) {
        let waiters = {
            let mut states = self.states.borrow_mut();
            states.in_flight.remove(set_id);
            states.waiters.remove(set_id)
        };
        for waker in waiters.unwrap_or_default() {
            waker.wake();
        }
    }

    /// セットの保存を止める。実行中の保存は終わるまで走る。
    pub(in crate::screens::record) fn stop(&self, set_id: &str) {
        self.states.borrow_mut().stopped.insert(set_id.to_owned());
    }

    /// 止めたセットの保存を再び許す。
    pub(in crate::screens::record) fn resume(&self, set_id: &str) {
        self.states.borrow_mut().stopped.remove(set_id);
    }

    /// セットの保存を止めているか。
    fn is_stopped(&self, set_id: &str) -> bool {
        self.states.borrow().stopped.contains(set_id)
    }

    /// セットの実行中の保存が終わるまで待つ。
    pub(in crate::screens::record) async fn wait_finished(&self, set_id: &str) {
        poll_fn(|context| {
            let mut states = self.states.borrow_mut();
            if !states.in_flight.contains(set_id) {
                return Poll::Ready(());
            }
            let waiters = states.waiters.entry(set_id.to_owned()).or_default();
            if !waiters.iter().any(|waker| waker.will_wake(context.waker())) {
                waiters.push(context.waker().clone());
            }
            Poll::Pending
        })
        .await;
    }
}

/// セットの入力の値を画面へ反映する。
fn update_set_input(
    mut views: Signal<Vec<ExerciseView>>,
    set_id: &str,
    segment_id: &str,
    field: Field,
    value: String,
) {
    let mut list = (views)();
    let Some(view) = list
        .iter_mut()
        .find(|view| view.sets.iter().any(|set| set.record.id == set_id))
    else {
        return;
    };
    let mode = view.snapshot.record_mode;
    let lateral = is_lateral(view.snapshot.laterality, mode);
    let Some(set) = view.sets.iter_mut().find(|set| set.record.id == set_id) else {
        return;
    };
    let Some(segment) = set
        .segments
        .iter_mut()
        .find(|segment| segment.record.id == segment_id)
    else {
        return;
    };
    match field {
        Field::Weight => segment.weight = value,
        Field::Reps => segment.reps = value,
        Field::Momentum => segment.momentum = value,
        Field::Assisted => segment.assisted = value,
        Field::Duration => segment.duration = value,
        Field::LeftWeight => segment.left_weight = value,
        Field::LeftReps => segment.left_reps = value,
        Field::LeftDuration => segment.left_duration = value,
        Field::RightWeight => segment.right_weight = value,
        Field::RightReps => segment.right_reps = value,
        Field::RightDuration => segment.right_duration = value,
    }
    if set.completed() && !set.values(lateral, mode).is_complete() {
        set.record.completed = 0;
    }
    set.bump();
    views.set(list);
}

/// セットの RPE を画面へ反映する。
fn update_set_rpe(mut views: Signal<Vec<ExerciseView>>, set_id: &str, value: String) {
    let mut list = (views)();
    let Some(set) = list
        .iter_mut()
        .flat_map(|view| view.sets.iter_mut())
        .find(|set| set.record.id == set_id)
    else {
        return;
    };
    if let Ok(value) = rpe::parse(&value) {
        set.record.rpe = value;
    }
    set.rpe = value;
    set.bump();
    views.set(list);
}

/// セット種別を画面へ反映する。ドロップセットの位置の規則を満たさない変更は拒む。
fn update_set_kind(
    mut views: Signal<Vec<ExerciseView>>,
    set_id: &str,
    kind: SetKind,
    error: Signal<RecordErrors>,
) -> bool {
    let mut list = (views)();
    let Some(view_index) = list
        .iter()
        .position(|view| view.sets.iter().any(|set| set.record.id == set_id))
    else {
        return false;
    };
    let Some(set_index) = list[view_index]
        .sets
        .iter()
        .position(|set| set.record.id == set_id)
    else {
        return false;
    };
    if !kind_allowed(&list[view_index].sets, set_index, kind) {
        set_set_error(error, set_id, Key::RecordSetDropInvalid);
        return false;
    }
    let set = &mut list[view_index].sets[set_index];
    set.record.kind = kind.as_str().to_owned();
    if kind != SetKind::Drop {
        set.segments.truncate(1);
    }
    set.bump();
    views.set(list);
    true
}

/// セットに段を追加して画面へ反映する。
fn append_segment(
    mut views: Signal<Vec<ExerciseView>>,
    set_id: &str,
    error: Signal<RecordErrors>,
) -> bool {
    let mut list = (views)();
    let Some(view) = list
        .iter_mut()
        .find(|view| view.sets.iter().any(|set| set.record.id == set_id))
    else {
        return false;
    };
    let lateral = is_lateral(view.snapshot.laterality, view.snapshot.record_mode);
    let mode = view.snapshot.record_mode;
    let Some(set) = view.sets.iter_mut().find(|set| set.record.id == set_id) else {
        return false;
    };
    if set.kind() != SetKind::Drop {
        return false;
    }
    let Ok(segment_id) = new_uuid() else {
        set_set_error(error, set_id, Key::RecordSetSaveFailed);
        return false;
    };
    let now = now_iso();
    let side_weight = match mode {
        RecordMode::BodyweightAndReps => Some(0.0),
        RecordMode::WeightAndReps | RecordMode::Time => None,
    };
    let side = lateral.then_some(SegmentSideValues {
        weight_kg: side_weight,
        reps: 0,
        duration_seconds: None,
    });
    set.record.completed = 0;
    set.segments.push(SegmentView {
        record: warrior_core::training_record::SetSegmentRecord {
            id: segment_id,
            set_id: set_id.to_owned(),
            position: i64::try_from(set.segments.len())
                .unwrap_or(i64::MAX)
                .saturating_add(1),
            weight_kg: Some(0.0),
            reps: Some(0),
            momentum_reps: 0,
            assisted_reps: 0,
            duration_seconds: None,
            left_weight_kg: side.and_then(|side| side.weight_kg),
            left_reps: side.map(|side| i64::from(side.reps)),
            left_duration_seconds: None,
            right_weight_kg: side.and_then(|side| side.weight_kg),
            right_reps: side.map(|side| i64::from(side.reps)),
            right_duration_seconds: None,
            created_at: now.clone(),
            updated_at: now,
        },
        weight: String::new(),
        reps: String::new(),
        momentum: String::new(),
        assisted: String::new(),
        duration: String::new(),
        left_weight: String::new(),
        left_reps: String::new(),
        left_duration: String::new(),
        right_weight: String::new(),
        right_reps: String::new(),
        right_duration: String::new(),
    });
    set.bump();
    views.set(list);
    true
}

/// 段を削除して画面へ反映する。残りの段が無いときはセットを削除する。
async fn delete_segment(
    training_id: String,
    set_id: String,
    segment_id: String,
    mut views: Signal<Vec<ExerciseView>>,
    error: Signal<RecordErrors>,
    saver: SetSaver,
) {
    let mut list = (views)();
    let Some(set) = list
        .iter_mut()
        .flat_map(|view| view.sets.iter_mut())
        .find(|set| set.record.id == set_id)
    else {
        return;
    };
    set.segments
        .retain(|segment| segment.record.id != segment_id);
    if set.segments.is_empty() {
        views.set(list);
        set_io::delete_set(training_id, set_id, views, error).await;
        return;
    }
    for (index, segment) in set.segments.iter_mut().enumerate() {
        segment.record.position = i64::try_from(index).unwrap_or(i64::MAX).saturating_add(1);
    }
    set.bump();
    views.set(list);
    set_io::save_set_loop(training_id, set_id, views, error, saver).await;
}

/// セットの完了の状態を変え、完了したらイベントを 1 回発行する。
///
/// 完了の書き込みは、そのセットの実行中の保存が終わってから行う。完了の後ろに
/// 値の更新を積むと、サーバーが完了済みのセットの更新を 409 で拒否するためである。
async fn complete_set(
    training_id: String,
    set_id: String,
    views: Signal<Vec<ExerciseView>>,
    error: Signal<RecordErrors>,
    saver: SetSaver,
    completed_events: SetCompletedEvents,
) {
    let Some((view, set)) = find_view_and_set(views, &set_id) else {
        return;
    };
    let completed = !set.completed();
    let lateral = is_lateral(view.snapshot.laterality, view.snapshot.record_mode);
    if completed && !set.can_complete(view.snapshot.record_mode, lateral) {
        set_set_error(
            error,
            &set_id,
            if view.snapshot.record_mode == RecordMode::Time {
                Key::RecordSetTimeInvalid
            } else if lateral {
                Key::RecordSetSidesInvalid
            } else {
                Key::RecordSetInputInvalid
            },
        );
        return;
    }
    let (updated, operation) = match training_set::complete(&training_id, &set.record, completed) {
        Ok(updated) => updated,
        Err(_) => {
            set_set_error(error, &set_id, Key::RecordSetSaveFailed);
            return;
        }
    };
    if completed {
        // 保存のループを止める前に画面の印を付けて、待つ間の入力を閉じる。
        update_completed(views, &set_id, true);
    }
    let storage = match storage::current().await {
        Ok(storage) => storage,
        Err(_) => {
            abort_completion(views, &set_id, completed, &saver, error);
            return;
        }
    };
    if completed {
        let values = set.values(lateral, view.snapshot.record_mode);
        if save_completed_set(&storage, &set, &values, &updated, &operation, &saver)
            .await
            .is_err()
        {
            abort_completion(views, &set_id, completed, &saver, error);
            return;
        }
    } else {
        update_completed(views, &set_id, false);
        if storage
            .save_set_complete(&updated, &operation)
            .await
            .is_err()
        {
            abort_completion(views, &set_id, completed, &saver, error);
            return;
        }
    }
    clear_set_error(error, &set_id);
    if completed {
        let starts_rest_timer = rest_timer_starts(
            view.superset
                .as_ref()
                .map(|group| group.member_ids.as_slice()),
            &view.record.id,
        );
        completed_events.publish(SetCompleted {
            training_id,
            training_exercise_id: view.record.id,
            set_id,
            starts_rest_timer,
        });
    } else {
        saver.resume(&set_id);
    }
    sync::flush_in_background();
}

/// 完了するセットの値の更新と完了を、保存の終わりを待ってこの順に保存する。
///
/// 時間で記録するセットのストップウォッチは、完了の前に止めて開始時刻を消す。
/// 完了したセットは編集できないため、開始時刻を残すと計測中の表示のまま戻せなくなる。
async fn save_completed_set(
    storage: &storage::Storage,
    set: &SetView,
    values: &SetValues,
    updated: &SetRecordRow,
    operation: &Operation,
    saver: &SetSaver,
) -> Result<(), storage::StorageError> {
    stop_saving_for_completion(saver, &set.record.id).await;
    save_completed_values(storage, &operation.training_id, set, values).await?;
    if values.mode == RecordMode::Time {
        clear_set_stopwatch(storage, &set.record.id).await?;
    }
    storage.save_set_complete(updated, operation).await
}

/// セットの全 side のストップウォッチの開始時刻を消す。
async fn clear_set_stopwatch(
    storage: &storage::Storage,
    set_id: &str,
) -> Result<(), storage::StorageError> {
    for side in [
        StopwatchSide::Whole,
        StopwatchSide::Left,
        StopwatchSide::Right,
    ] {
        storage.clear_stopwatch_started_at(set_id, side).await?;
    }
    Ok(())
}

/// セットの保存のループを止め、実行中の保存が終わるまで待つ。
async fn stop_saving_for_completion(saver: &SetSaver, set_id: &str) {
    saver.stop(set_id);
    saver.wait_finished(set_id).await;
}

/// 完了するセットの値の更新を端末内へ保存する。
async fn save_completed_values(
    storage: &storage::Storage,
    training_id: &str,
    set: &SetView,
    values: &SetValues,
) -> Result<(), storage::StorageError> {
    let segments = set
        .segments
        .iter()
        .map(|segment| segment.record.clone())
        .collect::<Vec<_>>();
    let (rows, operation) = training_set::update(training_id, &set.record, &segments, values)?;
    storage
        .save_set_update(&rows.set, &rows.segments, &operation)
        .await
}

/// 完了を書き込めなかったときに、画面の印と保存の停止を戻す。
fn abort_completion(
    views: Signal<Vec<ExerciseView>>,
    set_id: &str,
    completed: bool,
    saver: &SetSaver,
    error: Signal<RecordErrors>,
) {
    update_completed(views, set_id, !completed);
    if completed {
        saver.resume(set_id);
    }
    set_set_error(error, set_id, Key::RecordSetSaveFailed);
}

fn update_completed(mut views: Signal<Vec<ExerciseView>>, set_id: &str, completed: bool) {
    let mut list = (views)();
    for set in list.iter_mut().flat_map(|view| view.sets.iter_mut()) {
        if set.record.id != set_id {
            continue;
        }
        set.record.completed = if completed { 1 } else { 0 };
        set.bump();
    }
    views.set(list);
}

/// セットの順序を変え、端末内の位置と送信待ちの列へ保存する。
async fn move_set(
    training_id: String,
    set_id: String,
    delta: i32,
    views: Signal<Vec<ExerciseView>>,
    error: Signal<RecordErrors>,
) {
    let Some((view_id, ordered)) = move_set_order(views, &set_id, delta, error) else {
        return;
    };
    let operation = match training_set::reorder(&training_id, &view_id, &ordered) {
        Ok(operation) => operation,
        Err(_) => {
            set_set_error(error, &set_id, Key::RecordSetSaveFailed);
            return;
        }
    };
    let result = match storage::current().await {
        Ok(storage) => {
            storage
                .save_set_reorder(&training_id, &view_id, &ordered, &operation)
                .await
        }
        Err(error) => Err(error),
    };
    if result.is_err() {
        set_set_error(error, &set_id, Key::RecordSetSaveFailed);
        set_io::restore_sets(views, &view_id, error).await;
        return;
    }
    clear_set_error(error, &set_id);
    sync::flush_in_background();
}

fn move_set_order(
    mut views: Signal<Vec<ExerciseView>>,
    set_id: &str,
    delta: i32,
    error: Signal<RecordErrors>,
) -> Option<(String, Vec<String>)> {
    let mut list = (views)();
    let (view_id, ordered) = {
        let view = list
            .iter_mut()
            .find(|view| view.sets.iter().any(|set| set.record.id == set_id))?;
        let index = view.sets.iter().position(|set| set.record.id == set_id)?;
        let target = index as i32 + delta;
        if target < 0 || target as usize >= view.sets.len() {
            return None;
        }
        if !move_allowed(&view.sets, index, delta) {
            set_set_error(error, set_id, Key::RecordSetDropInvalid);
            return None;
        }
        view.sets.swap(index, target as usize);
        for (index, set) in view.sets.iter_mut().enumerate() {
            set.record.position = i64::try_from(index).unwrap_or(i64::MAX).saturating_add(1);
            set.bump();
        }
        (
            view.record.id.clone(),
            view.sets.iter().map(|set| set.record.id.clone()).collect(),
        )
    };
    views.set(list);
    Some((view_id, ordered))
}

fn find_view_and_set(
    views: Signal<Vec<ExerciseView>>,
    set_id: &str,
) -> Option<(ExerciseView, SetView)> {
    for view in (views)() {
        if let Some(set) = view.sets.iter().find(|set| set.record.id == set_id) {
            return Some((view.clone(), set.clone()));
        }
    }
    None
}

/// セットの操作を、種類ごとの非同期の処理へ振り分ける。
pub(in crate::screens::record) fn dispatch(
    action: SetAction,
    training_id: String,
    views: Signal<Vec<ExerciseView>>,
    error: Signal<RecordErrors>,
    saver: SetSaver,
    completed_events: SetCompletedEvents,
) {
    match action {
        SetAction::Add { view_id } => {
            spawn(set_io::add_set(training_id, view_id, views, error));
        }
        SetAction::ApplyProposal { view_id, proposal } => {
            spawn(set_io::apply_proposal(
                training_id,
                view_id,
                proposal,
                views,
                error,
            ));
        }
        SetAction::Copy { view_id } => {
            spawn(set_io::copy_sets(training_id, view_id, views, error));
        }
        SetAction::Input {
            set_id,
            segment_id,
            field,
            value,
        } => {
            update_set_input(views, &set_id, &segment_id, field, value);
            spawn(async move {
                set_io::save_set_loop(training_id, set_id, views, error, saver).await;
            });
        }
        SetAction::Rpe { set_id, value } => {
            update_set_rpe(views, &set_id, value);
            spawn(async move {
                set_io::save_set_loop(training_id, set_id, views, error, saver).await;
            });
        }
        SetAction::Grip { set_id, grip } => {
            spawn(grip::change_grip(
                training_id,
                set_id,
                grip,
                views,
                error,
                saver,
            ));
        }
        SetAction::Kind { set_id, kind } => {
            if update_set_kind(views, &set_id, kind, error) {
                spawn(async move {
                    set_io::save_set_loop(training_id, set_id, views, error, saver).await;
                });
            }
        }
        SetAction::AddSegment { set_id } => {
            if append_segment(views, &set_id, error) {
                spawn(async move {
                    set_io::save_set_loop(training_id, set_id, views, error, saver).await;
                });
            }
        }
        SetAction::DeleteSegment { set_id, segment_id } => {
            spawn(delete_segment(
                training_id,
                set_id,
                segment_id,
                views,
                error,
                saver,
            ));
        }
        SetAction::Complete { set_id } => {
            // ブラウザは利用者の操作の外で始まる音を止めることがある。完了の操作の中で
            // 音の再生を許可しておき、失敗しても完了は止めない。
            let _ = crate::sound::unlock();
            spawn(complete_set(
                training_id,
                set_id,
                views,
                error,
                saver,
                completed_events,
            ));
        }
        SetAction::Delete { set_id } => {
            spawn(set_io::delete_set(training_id, set_id, views, error));
        }
        SetAction::Move { set_id, delta } => {
            spawn(move_set(training_id, set_id, delta, views, error));
        }
    }
}

#[cfg(test)]
mod tests;
