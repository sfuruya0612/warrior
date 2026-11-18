//! 記録の種目の一覧、追加、並べ替え、削除と、前回の記録の表示、コピー、差、メモ。
//! スーパーセットの組の作成と解除もここから始める。
//!
//! 対応する要求: `REQ-REC-002`、`REQ-REC-003`、`REQ-REC-004`、`REQ-REC-007`、`REQ-SET-001`、
//! `REQ-SET-002`、`REQ-SET-005`、`REQ-SET-010`。
//! 種目の行とセットの行は端末内の IndexedDB に保存し、送信待ちの列を通す (ADR-002)。
//! セットの入力は画面へすぐ反映し、端末内の保存は入力の版が変わるたびに行う。
//! セットの完了は `SetCompleted` を 1 回発行する。前回の選択と差の計算は
//! `warrior_core::previous` の純粋な関数を使う。

use dioxus::prelude::*;
use warrior_core::duration;
use warrior_core::exercise::ExerciseSnapshot;
use warrior_core::previous::{
    DurationDifference, ExerciseDifference, PreviousExercise, PreviousTraining,
    TotalWeightDifference,
};
use warrior_core::sync::MEMO_MAX_CHARS;
use warrior_core::training_record::TrainingExerciseRecord;

use super::alternatives::AlternativePanel;
use super::errors::{clear_load_error, retain_set_errors, set_load_error, RecordErrors};
use super::exercise_view::{
    dispatch, dissolve_superset, exercise_diff, save_superset_group, training_difference,
    ExerciseView, SetSaver, SupersetGroupView,
};
use super::memo_save::{run_save_loop, MemoSaveState};
use super::proposals::ProposalPanel;
use super::rest_timer::{RestSecondsField, RestSecondsTarget};
use super::set_drawer::SetDrawer;
use super::set_events::SetCompletedEvents;
use super::set_table::{SetTable, SetTableState};
use super::sets::{
    default_grip, fill_missing_grips, format_number, set_kind_text, SelectedSet, SetAction, SetView,
};
use super::superset_panel::{SupersetCandidate, SupersetPanel};
use super::RecordSignals;
use crate::previous as previous_api;
use crate::record_rail::{RecordRailExercise, RecordRailState};
use crate::routes::navigate;
use crate::storage;
use crate::sync;
use crate::text::{text, Key};
use crate::training_exercise;

/// 記録の種目の一覧と、選んだ種目の詳細。
#[component]
pub(super) fn ExerciseSection(training_id: String, signals: RecordSignals) -> Element {
    let views = use_signal(Vec::<ExerciseView>::new);
    let selected_set = use_signal(|| Option::<String>::None);
    use_context_provider(|| SelectedSet(selected_set));
    let rail = use_context::<Signal<RecordRailState>>();
    let previous_training = use_signal(|| Option::<PreviousTraining>::None);
    let error = use_signal(RecordErrors::default);
    let loading = use_signal(|| true);
    let superset_busy = use_signal(|| false);
    let mut reload = use_signal(|| 0u32);
    let mut last_tree = use_signal(|| 0u64);
    let reload_effect = reload;
    let tree_revision = signals.tree_revision;
    let exercise_revision = signals.exercise_revision;
    let route_id = training_id.clone();
    use_effect(move || {
        let _ = (reload_effect)();
        let tree = (tree_revision)();
        let _ = (exercise_revision)();
        let preserve_inputs = tree == *last_tree.peek();
        last_tree.set(tree);
        let id = route_id.clone();
        spawn(load(
            id,
            views,
            rail,
            previous_training,
            error,
            loading,
            preserve_inputs,
        ));
    });
    let mut rail_for_drop = rail;
    use_drop(move || {
        let mut state = rail_for_drop.write();
        state.training_id = None;
        state.exercises.clear();
        state.selected = None;
    });

    let editable_completed =
        (signals.session)().is_some_and(|session| session.finished_at.is_some());
    let saver = use_hook(SetSaver::default);
    saver.set_editable_completed(editable_completed);
    let completed_events = use_context::<SetCompletedEvents>();
    let action_training_id = training_id.clone();
    let on_action = EventHandler::new(move |action: SetAction| {
        dispatch(
            action,
            action_training_id.clone(),
            views,
            error,
            saver.clone(),
            completed_events,
        );
    });

    let delete_training_id = training_id.clone();
    let on_delete = EventHandler::new(move |view_id: String| {
        let operation = match training_exercise::delete(&delete_training_id, &view_id) {
            Ok(operation) => operation,
            Err(_) => {
                set_load_error(error, Key::RecordExerciseSaveFailed);
                return;
            }
        };
        let training_id = delete_training_id.clone();
        spawn(async move {
            let result = match storage::current().await {
                Ok(storage) => {
                    storage
                        .save_training_exercise_delete(&training_id, &view_id, &operation)
                        .await
                }
                Err(error) => Err(error),
            };
            if result.is_err() {
                set_load_error(error, Key::RecordExerciseSaveFailed);
                return;
            }
            sync::flush_in_background();
            reload.set((reload)() + 1);
        });
    });
    let move_training_id = training_id.clone();
    let on_move = EventHandler::new(move |(view_id, delta): (String, i32)| {
        let mut list = (views)();
        let Some(index) = list.iter().position(|view| view.record.id == view_id) else {
            return;
        };
        let target = index as i32 + delta;
        if target < 0 || target as usize >= list.len() {
            return;
        }
        list.swap(index, target as usize);
        let ordered = list
            .iter()
            .map(|view| view.record.id.clone())
            .collect::<Vec<_>>();
        let training_id = move_training_id.clone();
        spawn(save_exercise_order(training_id, ordered, reload, error));
    });
    let defer_training_id = training_id.clone();
    let on_defer = EventHandler::new(move |view_id: String| {
        let mut list = (views)();
        let Some(index) = list.iter().position(|view| view.record.id == view_id) else {
            return;
        };
        let view = list.remove(index);
        list.push(view);
        let ordered = list
            .iter()
            .map(|view| view.record.id.clone())
            .collect::<Vec<_>>();
        let training_id = defer_training_id.clone();
        spawn(save_exercise_order(training_id, ordered, reload, error));
    });
    let add_training_id = training_id.clone();
    let on_add = EventHandler::new(move |_: ()| {
        navigate(&format!("/record/{add_training_id}/exercises"));
    });
    let on_changed = EventHandler::new(move |_: ()| reload.set((reload)() + 1));
    let superset_training_id = training_id.clone();
    let mut create_busy = superset_busy;
    let on_superset_create =
        EventHandler::new(move |(view_id, candidate_ids): (String, Vec<String>)| {
            if create_busy() {
                return;
            }
            create_busy.set(true);
            let training_id = superset_training_id.clone();
            spawn(async move {
                save_superset_group(training_id, view_id, candidate_ids, views, error).await;
                create_busy.set(false);
            });
        });
    let delete_superset_training_id = training_id.clone();
    let mut delete_busy = superset_busy;
    let on_superset_delete = EventHandler::new(move |superset_id: String| {
        if delete_busy() {
            return;
        }
        delete_busy.set(true);
        let training_id = delete_superset_training_id.clone();
        spawn(async move {
            dissolve_superset(training_id, superset_id, views, error).await;
            delete_busy.set(false);
        });
    });

    let current_views = (views)();
    let rail_state = (rail)();
    let selected_id = rail_state.selected;
    let selected_view = selected_id
        .as_ref()
        .and_then(|id| current_views.iter().find(|view| &view.record.id == id))
        .cloned();
    let selected_index = selected_view.as_ref().and_then(|selected| {
        current_views
            .iter()
            .position(|view| view.record.id == selected.record.id)
    });
    let view_only = (signals.view_only)();
    let training_diff = training_difference(&current_views, (previous_training)().as_ref());
    let errors = (error)();
    let load_error = errors.load;
    let set_errors = errors.sets;
    let is_loading = (loading)();
    let superset_candidates = selected_view.as_ref().map_or_else(Vec::new, |selected| {
        current_views
            .iter()
            .filter(|view| view.record.id != selected.record.id && view.superset.is_none())
            .map(|view| SupersetCandidate {
                training_exercise_id: view.record.id.clone(),
                exercise_id: view.snapshot.exercise_id.clone(),
                name: view.snapshot.name.clone(),
            })
            .collect::<Vec<_>>()
    });
    let superset_members = selected_view
        .as_ref()
        .and_then(|view| view.superset.as_ref())
        .map_or_else(Vec::new, |group| {
            group
                .member_ids
                .iter()
                .filter_map(|id| {
                    current_views
                        .iter()
                        .find(|view| &view.record.id == id)
                        .map(|view| (id.clone(), view.snapshot.name.clone()))
                })
                .collect::<Vec<_>>()
        });

    rsx! {
        section {
            class: "flex flex-col gap-2",
            "data-exercises": "true",
            div { class: "flex items-center justify-end gap-2",
                button {
                    class: "min-h-11 border border-primary px-3 text-[13px] font-bold text-primary",
                    "data-add-exercise": "true",
                    disabled: view_only,
                    onclick: move |_| on_add.call(()),
                    "{text(Key::RecordExerciseAdd)}"
                }
            }
            if let Some(message) = load_error {
                p {
                    class: "text-[13px] text-primary",
                    "data-record-error": "true",
                    "{message}"
                }
                button {
                    class: "min-h-11 border border-line px-3 text-[13px]",
                    onclick: move |_| reload.set((reload)() + 1),
                    "{text(Key::RecordRetry)}"
                }
            }
            for (set_id, message) in set_errors {
                p {
                    class: "text-[13px] text-primary",
                    "data-record-error": "true",
                    "data-record-error-set-id": "{set_id}",
                    "{message}"
                }
            }
            if current_views.is_empty() && !is_loading {
                p { class: "text-[13px] text-text-sub", "{text(Key::RecordExerciseEmpty)}" }
            }
            if let Some(view) = selected_view {
                {
                    let first = selected_index == Some(0);
                    let last = selected_index
                        .is_some_and(|index| index + 1 == current_views.len());
                    let up_id = view.record.id.clone();
                    let down_id = view.record.id.clone();
                    let delete_id = view.record.id.clone();
                    let defer_id = view.record.id.clone();
                    let drawer_mode = view.snapshot.record_mode;
                    let drawer_grip_mode = view.snapshot.grip_mode;
                    let drawer_grip_options = view.snapshot.grip_options.clone();
                    let drawer_rows = view.sets.clone();
                    let detail_id = view.record.id.clone();
                    rsx! {
                        ExerciseDetail {
                            key: "{detail_id}",
                            state: ExerciseDetailState {
                                training_id: training_id.clone(),
                                view,
                                disabled: view_only || is_loading || superset_busy(),
                                editable_completed,
                                first,
                                last,
                                superset_members,
                                superset_candidates,
                                memo_revision: signals.exercise_revision,
                                tree_revision: signals.tree_revision,
                            },
                            actions: ExerciseDetailActions {
                                on_action,
                                on_up: EventHandler::new(move |_: ()| {
                                    on_move.call((up_id.clone(), -1));
                                }),
                                on_down: EventHandler::new(move |_: ()| {
                                    on_move.call((down_id.clone(), 1));
                                }),
                                on_defer: EventHandler::new(move |_: ()| {
                                    on_defer.call(defer_id.clone());
                                }),
                                on_delete: EventHandler::new(move |_: ()| {
                                    on_delete.call(delete_id.clone());
                                }),
                                on_superset_create,
                                on_superset_delete,
                                on_changed,
                            },
                        }
                        SetDrawer {
                            rows: drawer_rows,
                            mode: drawer_mode,
                            grip_mode: drawer_grip_mode,
                            grip_options: drawer_grip_options,
                            disabled: view_only,
                            on_action,
                        }
                    }
                }
            } else {
                EmptySetTable {}
            }
            if let Some(diff) = training_diff {
                TrainingDifference { diff }
            }
        }
    }
}

async fn save_exercise_order(
    training_id: String,
    ordered: Vec<String>,
    mut reload: Signal<u32>,
    error: Signal<RecordErrors>,
) {
    let operation = match training_exercise::reorder(&training_id, &ordered) {
        Ok(operation) => operation,
        Err(_) => {
            set_load_error(error, Key::RecordExerciseSaveFailed);
            return;
        }
    };
    let result = match storage::current().await {
        Ok(storage) => {
            storage
                .save_training_exercise_reorder(&training_id, &ordered, &operation)
                .await
        }
        Err(error) => Err(error),
    };
    if result.is_err() {
        set_load_error(error, Key::RecordExerciseSaveFailed);
        return;
    }
    sync::flush_in_background();
    reload.set((reload)() + 1);
}

async fn load(
    training_id: String,
    mut views: Signal<Vec<ExerciseView>>,
    mut rail: Signal<RecordRailState>,
    mut previous_training: Signal<Option<PreviousTraining>>,
    error: Signal<RecordErrors>,
    mut loading: Signal<bool>,
    preserve_inputs: bool,
) {
    loading.set(true);
    let storage = match storage::current().await {
        Ok(storage) => storage,
        Err(_) => {
            set_load_error(error, Key::RecordExerciseLoadFailed);
            loading.set(false);
            return;
        }
    };
    let records = match storage.training_exercises(&training_id).await {
        Ok(records) => records,
        Err(_) => {
            set_load_error(error, Key::RecordExerciseLoadFailed);
            loading.set(false);
            return;
        }
    };
    let body_weight_kg = crate::profile::body_weight_for_record().await;
    let supersets = match storage.training_supersets(&training_id).await {
        Ok(supersets) => supersets,
        Err(_) => {
            set_load_error(error, Key::RecordSupersetLoadFailed);
            loading.set(false);
            return;
        }
    };
    let old = (views)();
    let mut next = Vec::with_capacity(records.len());
    let mut training_previous = match previous_api::fetch_training(&training_id).await {
        Ok(response) => response.training,
        Err(_) => None,
    };
    for record in records {
        let Ok(snapshot) = serde_json::from_str::<ExerciseSnapshot>(&record.snapshot) else {
            continue;
        };
        let stored_sets = match storage.training_sets(&record.id).await {
            Ok(sets) => sets,
            Err(_) => {
                set_load_error(error, Key::RecordSetLoadFailed);
                loading.set(false);
                return;
            }
        };
        let loaded_sets = stored_sets
            .into_iter()
            .filter_map(SetView::from_stored)
            .collect::<Vec<_>>();
        let mut sets = if preserve_inputs {
            old.iter()
                .find(|view| view.record.id == record.id)
                .map_or(loaded_sets, |view| view.sets.clone())
        } else {
            loaded_sets
        };
        fill_missing_grips(&mut sets, snapshot.grip_mode, &snapshot.grip_options);
        let latest_grip = sets.last().and_then(SetView::grip);
        let selection = default_grip(snapshot.grip_mode, &snapshot.grip_options, latest_grip);
        let (grip, attachment) = previous_api::grip_query(selection);
        let (previous, previous_error) = match previous_api::fetch(
            &training_id,
            &snapshot.exercise_id,
            grip.as_deref(),
            attachment.as_deref(),
        )
        .await
        {
            Ok(response) => {
                if training_previous.is_none() {
                    training_previous = response.training.clone();
                }
                (response.exercise, false)
            }
            Err(_) => (None, true),
        };
        let superset = supersets
            .iter()
            .find(|group| {
                group
                    .members
                    .iter()
                    .any(|member| member.training_exercise_id == record.id)
            })
            .map(|group| SupersetGroupView {
                id: group.id.clone(),
                member_ids: group
                    .members
                    .iter()
                    .map(|member| member.training_exercise_id.clone())
                    .collect(),
            });
        next.push(ExerciseView {
            record,
            snapshot,
            previous,
            previous_error,
            sets,
            body_weight_kg,
            superset,
        });
    }
    let current_selected = if rail.peek().training_id.as_deref() == Some(training_id.as_str()) {
        rail.peek().selected.clone()
    } else {
        None
    };
    let selected = current_selected
        .filter(|id| next.iter().any(|view| &view.record.id == id))
        .or_else(|| next.first().map(|view| view.record.id.clone()));
    rail.set(RecordRailState {
        training_id: Some(training_id.clone()),
        exercises: next
            .iter()
            .map(|view| RecordRailExercise {
                id: view.record.id.clone(),
                exercise_id: view.snapshot.exercise_id.clone(),
                name: view.snapshot.name.clone(),
            })
            .collect(),
        selected,
    });
    let set_ids = next
        .iter()
        .flat_map(|view| view.sets.iter().map(|set| set.record.id.clone()))
        .collect::<Vec<_>>();
    previous_training.set(training_previous);
    views.set(next);
    clear_load_error(error);
    retain_set_errors(error, &set_ids);
    loading.set(false);
}

/// 種目の詳細に渡す状態。
#[derive(Clone, PartialEq)]
struct ExerciseDetailState {
    /// 対象のトレーニングの ID。
    training_id: String,
    /// 種目の状態。
    view: ExerciseView,
    /// 編集できないか。
    disabled: bool,
    /// 終了したトレーニングで、完了済みのセットを編集できるか。
    editable_completed: bool,
    /// 先頭の種目か。
    first: bool,
    /// 末尾の種目か。
    last: bool,
    /// 組の種目と名前。
    superset_members: Vec<(String, String)>,
    /// 組に加えられる種目。
    superset_candidates: Vec<SupersetCandidate>,
    /// 種目のメモを外部から戻した版。
    memo_revision: Signal<u64>,
    /// 記録全体を読み直した版。
    tree_revision: Signal<u64>,
}

/// 種目の詳細の操作。
#[derive(Clone, PartialEq)]
struct ExerciseDetailActions {
    /// セットの操作。
    on_action: EventHandler<SetAction>,
    /// 前へ移動する。
    on_up: EventHandler<()>,
    /// 後ろへ移動する。
    on_down: EventHandler<()>,
    /// 一覧の後ろへ回す。
    on_defer: EventHandler<()>,
    /// 削除する。
    on_delete: EventHandler<()>,
    /// 組を作る。
    on_superset_create: EventHandler<(String, Vec<String>)>,
    /// 組を解除する。
    on_superset_delete: EventHandler<String>,
    /// 種目を入れ替えた後に記録を読み直す。
    on_changed: EventHandler<()>,
}

#[component]
fn ExerciseDetail(state: ExerciseDetailState, actions: ExerciseDetailActions) -> Element {
    let ExerciseDetailState {
        training_id,
        view,
        disabled,
        editable_completed,
        first,
        last,
        superset_members,
        superset_candidates,
        memo_revision,
        tree_revision,
    } = state;
    let ExerciseDetailActions {
        on_action,
        on_up,
        on_down,
        on_defer,
        on_delete,
        on_superset_create,
        on_superset_delete,
        on_changed,
    } = actions;
    let diff = exercise_diff(&view);
    let view_id = view.record.id.clone();
    let copy_id = view.record.id.clone();
    let previous = view.previous.clone();
    let previous_error = view.previous_error;
    let mode = view.snapshot.record_mode;
    let laterality = view.snapshot.laterality;
    let superset_view_id = view.record.id.clone();
    let rest_exercise_id = view.record.id.clone();
    let proposal_view_id = view.record.id.clone();
    let proposal_exercise_id = view.snapshot.exercise_id.clone();
    let superset = view.superset.clone();
    let rows = view.sets.clone();
    let has_completed = rows.iter().any(SetView::completed);

    rsx! {
        div {
            class: "flex flex-col gap-2 border border-line bg-surface p-2",
            "data-exercise-detail": "true",
            "data-exercise-id": "{view.snapshot.exercise_id}",
            div { class: "flex flex-wrap items-center justify-between gap-1",
                h3 { class: "text-[13px] font-bold", "{view.snapshot.name}" }
                div { class: "flex flex-wrap gap-1",
                    button {
                        class: "min-h-11 border border-line px-2 text-[11px]",
                        "data-move-up": "true",
                        disabled: disabled || first,
                        onclick: move |_| on_up.call(()),
                        "{text(Key::RecordExerciseMoveUp)}"
                    }
                    button {
                        class: "min-h-11 border border-line px-2 text-[11px]",
                        "data-move-down": "true",
                        disabled: disabled || last,
                        onclick: move |_| on_down.call(()),
                        "{text(Key::RecordExerciseMoveDown)}"
                    }
                    button {
                        class: "min-h-11 border border-line px-2 text-[11px]",
                        "data-defer-exercise": "true",
                        disabled: disabled || last,
                        onclick: move |_| on_defer.call(()),
                        "{text(Key::RecordExerciseDefer)}"
                    }
                    button {
                        class: "min-h-11 border border-line px-2 text-[11px] text-primary",
                        "data-delete-exercise": "true",
                        disabled: disabled || (has_completed && !editable_completed),
                        onclick: move |_| on_delete.call(()),
                        "{text(Key::RecordExerciseDelete)}"
                    }
                    if previous.is_some() {
                        button {
                            class: "min-h-11 border border-primary px-2 text-[11px] font-bold text-primary",
                            "data-copy-previous": "true",
                            disabled: disabled || has_completed,
                            onclick: move |_| on_action.call(SetAction::Copy { view_id: copy_id.clone() }),
                            "{text(Key::RecordExerciseCopy)}"
                        }
                    }
                }
            }
            if let Some(previous) = previous {
                PreviousRecord { previous }
            } else if previous_error {
                p { class: "text-[11px] text-text-sub", "{text(Key::RecordExercisePreviousFailed)}" }
            }
            AlternativePanel {
                record: view.record.clone(),
                snapshot: view.snapshot.clone(),
                disabled,
                has_completed,
                on_changed,
            }
            ProposalPanel {
                exercise_id: proposal_exercise_id,
                disabled,
                on_apply: move |proposal| {
                    on_action.call(SetAction::ApplyProposal {
                        view_id: proposal_view_id.clone(),
                        proposal,
                    });
                },
            }
            ExerciseMemoEditor {
                training_id: training_id.clone(),
                record: view.record.clone(),
                disabled,
                revision: memo_revision,
                tree_revision,
            }
            SupersetPanel {
                view_id: superset_view_id,
                group: superset,
                members: superset_members,
                candidates: superset_candidates,
                disabled,
                on_create: on_superset_create,
                on_delete: on_superset_delete,
            }
            RestSecondsField {
                target: RestSecondsTarget::Exercise(rest_exercise_id),
                disabled,
            }
            if let Some(diff) = diff {
                ExerciseDifferenceView { diff }
            }
            SetTable {
                state: SetTableState {
                    view_id,
                    rows,
                    mode,
                    laterality,
                    disabled,
                    editable_completed,
                },
                on_action,
            }
        }
    }
}

#[component]
fn ExerciseMemoEditor(
    training_id: String,
    record: TrainingExerciseRecord,
    disabled: bool,
    revision: Signal<u64>,
    tree_revision: Signal<u64>,
) -> Element {
    let saving = use_signal(|| false);
    let initial = record.memo.clone();
    let state = use_hook(|| MemoSaveState {
        saving,
        in_flight: std::rc::Rc::new(std::cell::Cell::new(false)),
        dirty: std::rc::Rc::new(std::cell::Cell::new(false)),
        draft: std::rc::Rc::new(std::cell::RefCell::new(initial)),
    });
    let error = use_signal(|| Option::<String>::None);
    let input_state = state.clone();
    let retry_state = state.clone();
    let reset_state = state.clone();
    let reset_memo = record.memo.clone();
    let mut reset_error = error;

    use_effect(move || {
        let _ = (revision)();
        let _ = (tree_revision)();
        *reset_state.draft.borrow_mut() = reset_memo.clone();
        reset_error.set(None);
    });

    let revision_key = format!("{}-{}", (revision)(), (tree_revision)());
    let memo = record.memo.clone();
    let input_training_id = training_id.clone();
    let input_exercise_id = record.id.clone();
    let retry_training_id = training_id.clone();
    let retry_exercise_id = record.id.clone();
    let saving_now = (state.saving)();
    let error_message = (error)();
    let field_id = format!("record-exercise-memo-{}", record.id);

    rsx! {
        div {
            key: "{revision_key}",
            class: "flex flex-col gap-2",
            label { class: "text-[11px] font-bold", r#for: "{field_id}", "{text(Key::RecordExerciseMemoLabel)}" }
            textarea {
                id: "{field_id}",
                class: "min-h-16 border border-line bg-bg p-2 text-[13px] leading-5",
                "data-exercise-memo": "true",
                "data-exercise-memo-id": "{record.id}",
                initial_value: "{memo}",
                disabled,
                maxlength: "{MEMO_MAX_CHARS}",
                placeholder: "{text(Key::RecordExerciseMemoPlaceholder)}",
                oninput: move |event| {
                    *input_state.draft.borrow_mut() = event.value();
                    if !input_state.request() {
                        return;
                    }
                    spawn(save_exercise_memo(
                        input_training_id.clone(),
                        input_exercise_id.clone(),
                        input_state.clone(),
                        error,
                    ));
                },
            }
            if saving_now {
                p { class: "text-[11px] text-text-sub", "{text(Key::RecordSaving)}" }
            }
            if let Some(message) = error_message {
                p { class: "text-[11px] text-primary", "{message}" }
                button {
                    class: "min-h-11 border border-line px-2 text-[11px]",
                    onclick: move |_| {
                        if retry_state.in_flight.get() {
                            return;
                        }
                        retry_state.dirty.set(true);
                        retry_state.in_flight.set(true);
                        spawn(save_exercise_memo(
                            retry_training_id.clone(),
                            retry_exercise_id.clone(),
                            retry_state.clone(),
                            error,
                        ));
                    },
                    "{text(Key::RecordRetry)}"
                }
            }
        }
    }
}

async fn save_exercise_memo(
    training_id: String,
    training_exercise_id: String,
    state: MemoSaveState,
    mut error: Signal<Option<String>>,
) {
    run_save_loop(state, move |memo| {
        let training_id = training_id.clone();
        let training_exercise_id = training_exercise_id.clone();
        async move {
            let operation =
                match training_exercise::memo(&training_id, &training_exercise_id, &memo) {
                    Ok(operation) => operation,
                    Err(_) => {
                        error.set(Some(text(Key::RecordExerciseSaveFailed).to_owned()));
                        return false;
                    }
                };
            let result = match storage::current().await {
                Ok(storage) => {
                    storage
                        .save_training_exercise_memo(
                            &training_id,
                            &training_exercise_id,
                            &memo,
                            &operation,
                        )
                        .await
                }
                Err(error) => Err(error),
            };
            match result {
                Ok(()) => {
                    error.set(None);
                    sync::flush_in_background();
                    true
                }
                Err(_) => {
                    error.set(Some(text(Key::RecordExerciseSaveFailed).to_owned()));
                    false
                }
            }
        }
    })
    .await;
}

#[component]
fn PreviousRecord(previous: PreviousExercise) -> Element {
    rsx! {
        div {
            class: "flex flex-col gap-1 border border-line p-2",
            "data-previous": "true",
            "data-previous-total-weight-kg": "{previous.total_weight_kg}",
            "data-previous-total-duration-seconds": "{previous.total_duration_seconds}",
            "data-previous-max-duration-seconds": "{previous.max_duration_seconds}",
            h4 { class: "text-[11px] font-bold", "{text(Key::RecordExercisePrevious)}" }
            for (index, set) in previous.sets.iter().enumerate() {
                div {
                    class: "text-[11px] leading-5",
                    "data-previous-set": "true",
                    "data-set-index": "{index}",
                    "data-set-kind": "{set.kind.as_str()}",
                    "{previous_set_text(index, set)}"
                }
            }
        }
    }
}

#[component]
fn ExerciseDifferenceView(diff: ExerciseDifference) -> Element {
    match diff {
        ExerciseDifference::Weight(diff) => rsx! {
            div {
                class: "text-[13px] font-bold",
                "data-exercise-weight-diff": "{rounded_tenth(diff.difference_kg)}",
                "{text(Key::RecordExerciseWeightDiff)}: {signed_weight(diff.difference_kg)}"
            }
        },
        ExerciseDifference::Time(diff) => rsx! {
            div {
                class: "flex flex-col gap-1 text-[13px] font-bold",
                "data-exercise-total-duration-diff": "{diff.total_difference_seconds}",
                "data-exercise-longest-duration-diff": "{diff.longest_difference_seconds}",
                div { "{text(Key::RecordExerciseTotalDurationDiff)}: {signed_duration(diff)}" }
                div { "{text(Key::RecordExerciseLongestDurationDiff)}: {signed_longest(diff)}" }
            }
        },
    }
}

#[component]
fn TrainingDifference(diff: TotalWeightDifference) -> Element {
    rsx! {
        div {
            class: "border border-line bg-surface p-2 text-[13px] font-bold",
            "data-training-total-diff": "{rounded_tenth(diff.difference_kg)}",
            "{text(Key::RecordTrainingWeightDiff)}: {signed_weight(diff.difference_kg)}"
        }
    }
}

#[component]
fn EmptySetTable() -> Element {
    rsx! {
        div { class: "flex flex-col gap-1",
            h2 { class: "text-sm font-bold", "{text(Key::RecordSets)}" }
            div {
                class: "h-[222px] overflow-y-auto border border-line bg-surface",
                "data-set-table": "true",
                "data-set-rows": "5",
                "data-set-row-height": "44",
                for index in 0..5 {
                    div {
                        class: "h-11 border-b border-line",
                        "data-set-row": "true",
                        "data-set-row-index": "{index}",
                    }
                }
            }
        }
    }
}

fn previous_set_text(index: usize, set: &warrior_core::previous::PreviousSet) -> String {
    let segment = set.segments.first();
    let value = match segment {
        Some(segment) if segment.longest_duration_seconds() > 0 => previous_duration_text(segment),
        Some(segment) => {
            let side_text = |side: &warrior_core::set_calc::SideValue| {
                format!("{} kg x {}", format_number(side.weight_kg), side.reps)
            };
            match (segment.left.as_ref(), segment.right.as_ref()) {
                (Some(left), Some(right)) => format!(
                    "{} {} / {} {}",
                    text(Key::RecordSetLeft),
                    side_text(left),
                    text(Key::RecordSetRight),
                    side_text(right)
                ),
                _ => format!("{} kg x {}", format_number(segment.weight_kg), segment.reps),
            }
        }
        None => String::new(),
    };
    format!("{} {} ({})", index + 1, value, set_kind_text(set.kind))
}

fn previous_duration_text(segment: &warrior_core::set_calc::SetSegment) -> String {
    let side_duration = |side: &warrior_core::set_calc::SideValue| {
        side.duration_seconds
            .map_or_else(|| "00:00".to_owned(), duration::format)
    };
    match (segment.left.as_ref(), segment.right.as_ref()) {
        (Some(left), Some(right)) => format!(
            "{} {} / {} {}",
            text(Key::RecordSetLeft),
            side_duration(left),
            text(Key::RecordSetRight),
            side_duration(right)
        ),
        _ => duration::format(segment.longest_duration_seconds()),
    }
}

fn signed_weight(value: f64) -> String {
    let value = rounded_tenth(value);
    if value > 0.0 {
        format!("+{value:.1} kg")
    } else if value < 0.0 {
        format!("{value:.1} kg")
    } else {
        "0 kg".to_owned()
    }
}

fn rounded_tenth(value: f64) -> f64 {
    if !value.is_finite() {
        return value;
    }
    let rounded = (value * 10.0).round() / 10.0;
    if rounded == 0.0 {
        0.0
    } else {
        rounded
    }
}

fn signed_duration(diff: DurationDifference) -> String {
    signed_seconds(diff.total_difference_seconds)
}

fn signed_longest(diff: DurationDifference) -> String {
    signed_seconds(diff.longest_difference_seconds)
}

fn signed_seconds(value: i64) -> String {
    if value > 0 {
        format!("+{value} {}", text(Key::RecordSeconds))
    } else if value < 0 {
        format!("{value} {}", text(Key::RecordSeconds))
    } else {
        format!("0 {}", text(Key::RecordSeconds))
    }
}
