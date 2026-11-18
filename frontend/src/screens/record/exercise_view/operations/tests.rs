//! セットの保存と完了の順序のブラウザのテスト。
//!
//! 対応する要求: `REQ-SET-009`、`REQ-REC-006`。セットの完了は、保存中の値の更新より後ろに積まない。
//! 終了したトレーニングでは、完了済みのセットも編集して保存できる。

use std::future::Future;
use std::pin::pin;
use std::task::{Context, Waker};

use warrior_core::exercise::RecordMode;
use warrior_core::set_calc::SetKind;
use warrior_core::sync::OperationKind;
use wasm_bindgen_test::wasm_bindgen_test;
use wasm_bindgen_test::wasm_bindgen_test_configure;

use super::set_io::completed_can_save;
use super::{save_completed_set, stop_saving_for_completion, SetSaver, SetView};
use crate::storage::{Storage, SCHEMA_VERSION};
use crate::training_set::{self, SetSegmentValues, SetValues};

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn a_stopped_set_does_not_start_saving() {
    let saver = SetSaver::default();
    saver.stop("set");
    assert!(!saver.start("set"));
    assert!(saver.is_stopped("set"));
    saver.resume("set");
    assert!(!saver.is_stopped("set"));
    assert!(saver.start("set"));
}

#[wasm_bindgen_test]
fn waiting_for_a_set_ends_when_its_saving_ends() {
    let saver = SetSaver::default();
    assert!(saver.start("set"));
    let mut waiting = pin!(saver.wait_finished("set"));
    let mut context = Context::from_waker(Waker::noop());
    assert!(waiting.as_mut().poll(&mut context).is_pending());
    saver.finish("set");
    assert!(waiting.as_mut().poll(&mut context).is_ready());
}

#[wasm_bindgen_test]
fn waiting_for_an_idle_set_ends_at_once() {
    let saver = SetSaver::default();
    let mut waiting = pin!(saver.wait_finished("set"));
    let mut context = Context::from_waker(Waker::noop());
    assert!(waiting.as_mut().poll(&mut context).is_ready());
}

#[wasm_bindgen_test]
fn a_completed_set_saves_only_while_the_training_is_finished() {
    let values = SetValues {
        kind: SetKind::Normal,
        mode: RecordMode::WeightAndReps,
        segments: vec![SetSegmentValues {
            weight_kg: 100.0,
            reps: 5,
            momentum_reps: 0,
            assisted_reps: 0,
            duration_seconds: None,
            left: None,
            right: None,
        }],
        completed: true,
        body_weight_kg: Some(70.0),
        rpe: None,
        grip: None,
    };
    let (mut rows, _) = training_set::create("training", "exercise", 1, &values)
        .expect("the completed set creation");
    rows.set.completed = 1;
    let set = SetView::from_rows(rows.set, rows.segments);
    assert!(set.completed());

    let saver = SetSaver::default();
    assert!(!completed_can_save(&saver, &set));
    saver.set_editable_completed(true);
    assert!(completed_can_save(&saver, &set));
}

#[wasm_bindgen_test]
fn a_saving_set_does_not_start_twice() {
    let saver = SetSaver::default();
    assert!(saver.start("set"));
    assert!(!saver.start("set"));
    assert!(saver.start("other"));
    saver.finish("set");
    assert!(saver.start("set"));
}

#[wasm_bindgen_test]
async fn completing_a_set_waits_for_the_running_save() {
    let saver = SetSaver::default();
    assert!(saver.start("set"));
    let mut waiting = pin!(stop_saving_for_completion(&saver, "set"));
    let mut context = Context::from_waker(Waker::noop());
    assert!(waiting.as_mut().poll(&mut context).is_pending());
    assert!(saver.is_stopped("set"));
    saver.finish("set");
    assert!(waiting.as_mut().poll(&mut context).is_ready());
}

#[wasm_bindgen_test]
async fn completing_a_set_saves_the_values_before_the_completion() {
    let name = format!(
        "warrior-test-completion-order-{}",
        crate::ids::new_uuid().expect("a database name")
    );
    let storage = Storage::open_with(&name, SCHEMA_VERSION)
        .await
        .expect("the database must open");
    let values = SetValues {
        kind: SetKind::Normal,
        mode: RecordMode::WeightAndReps,
        segments: vec![SetSegmentValues {
            weight_kg: 100.0,
            reps: 5,
            momentum_reps: 0,
            assisted_reps: 0,
            duration_seconds: None,
            left: None,
            right: None,
        }],
        completed: false,
        body_weight_kg: Some(70.0),
        rpe: None,
        grip: None,
    };
    let (rows, create) =
        training_set::create("training", "exercise", 1, &values).expect("the set creation");
    storage
        .save_set_create(&rows.set, &rows.segments, &create)
        .await
        .expect("the set must be saved");

    let set = SetView::from_rows(rows.set.clone(), rows.segments.clone());
    let values = set.values(false, RecordMode::WeightAndReps);
    let (updated, complete) =
        training_set::complete("training", &rows.set, true).expect("the completion");
    save_completed_set(
        &storage,
        &set,
        &values,
        &updated,
        &complete,
        &SetSaver::default(),
    )
    .await
    .expect("the completion must be saved");

    let kinds = storage
        .outbox_entries()
        .await
        .expect("the outbox")
        .into_iter()
        .map(|entry| entry.operation.kind)
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![
            OperationKind::SetCreate,
            OperationKind::SetUpdate,
            OperationKind::SetComplete,
        ],
        "the values must be saved before the completion"
    );
    let stored = storage
        .training_sets("exercise")
        .await
        .expect("the sets must be readable");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].set.completed, 1);
    assert_eq!(stored[0].segments[0].weight_kg, Some(100.0));
    assert_eq!(stored[0].segments[0].reps, Some(5));

    storage
        .clear_account()
        .await
        .expect("the cleanup must work");
}
