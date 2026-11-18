//! カレンダーの月表示、部位の色分け、日数の集計、日の詳細、凡例。

use dioxus::prelude::*;
use dioxus_router::Link;
use warrior_core::calendar::{
    next_month, previous_month, CalendarDayDetail, CalendarExercise, CalendarMonth, CalendarSet,
};
use warrior_core::exercise::BodyPart;
use warrior_core::jst::{JstDate, JstMonth, UtcInstant};

use crate::calendar as calendar_api;
use crate::calendar_view::{
    build_cells, day_aria_label, day_cell_class, day_number_class, day_parts, day_title,
    format_weight, part_color_class, part_counts, part_label, set_notes, set_text, weekdays,
    CalendarCell,
};
use crate::date_select::DateSelect;
use crate::ids::{new_uuid, now_iso};
use crate::routes::Route;
use crate::screens::history::{backfill_operation, backfill_session};
use crate::storage;
use crate::sync;
use crate::text::{text, Key};

#[component]
pub fn CalendarScreen() -> Element {
    let today = current_date();
    let state = use_calendar_state(today);
    let handlers = calendar_handlers(state);

    let month_now = (state.month)();
    let selected = (state.selected)();
    let view = (state.month_data)();
    let month_error = (state.month_error)();
    let detail = (state.day_data)();
    let day_error = (state.day_error)();
    let training_days = view.as_ref().map(|view| view.training_days).unwrap_or(0);
    let part_days = part_counts(view.as_ref());
    let loading = month_now.is_some() && view.is_none() && month_error.is_none();
    let detail_loading = selected.is_some() && detail.is_none() && day_error.is_none();

    rsx! {
        div {
            class: "min-h-full",
            "data-calendar": "true",
            "data-calendar-month": month_now.map(month_text).unwrap_or_default(),
            section { class: "px-3.5",
                div { class: "flex h-12 items-center justify-between",
                    h1 { class: "text-[17px] font-black", "{text(Key::CalendarTitle)}" }
                    Link {
                        to: Route::History {},
                        class: "flex min-h-11 items-center border border-line px-2 text-[11px] text-primary",
                        "data-calendar-history": "true",
                        "{text(Key::CalendarHistory)}"
                    }
                }
                MonthHeading {
                    month: month_now,
                    training_days: training_days,
                    on_previous: handlers.previous,
                    on_next: handlers.next,
                }
                MonthGrid {
                    month: month_now,
                    view: view,
                    selected: selected,
                    today: today,
                    on_select: handlers.select,
                }
                PartLegend { counts: part_days }
                DayDetail {
                    detail: detail,
                    selected: selected,
                    error: day_error,
                    loading: detail_loading,
                    on_retry: handlers.day_retry,
                }
                CalendarAddForm { state: state, today: today }
                MonthStatus {
                    loading: loading,
                    error: month_error,
                    on_retry: handlers.month_retry,
                }
            }
        }
    }
}

/// カレンダー画面の状態。月と日の読み込みも、この状態へ書き込む。
#[derive(Clone, Copy, PartialEq)]
struct CalendarSignals {
    month: Signal<Option<JstMonth>>,
    selected: Signal<Option<JstDate>>,
    month_data: Signal<Option<CalendarMonth>>,
    month_error: Signal<Option<String>>,
    day_data: Signal<Option<CalendarDayDetail>>,
    day_error: Signal<Option<String>>,
    month_reload: Signal<u32>,
    day_reload: Signal<u32>,
    month_generation: Signal<u32>,
    day_generation: Signal<u32>,
}

/// カレンダー画面の状態を作り、月と日の読み込みを始める効果を登録する。
fn use_calendar_state(today: Option<JstDate>) -> CalendarSignals {
    let state = CalendarSignals {
        month: use_signal(|| today.map(|date| date.month_range())),
        selected: use_signal(|| Option::<JstDate>::None),
        month_data: use_signal(|| Option::<CalendarMonth>::None),
        month_error: use_signal(|| Option::<String>::None),
        day_data: use_signal(|| Option::<CalendarDayDetail>::None),
        day_error: use_signal(|| Option::<String>::None),
        month_reload: use_signal(|| 0u32),
        day_reload: use_signal(|| 0u32),
        month_generation: use_signal(|| 0u32),
        day_generation: use_signal(|| 0u32),
    };

    use_effect(move || {
        let mut state = state;
        let _ = (state.month_reload)();
        let Some(current) = (state.month)() else {
            return;
        };
        let generation = state.month_generation.peek().wrapping_add(1);
        state.month_generation.set(generation);
        state.month_error.set(None);
        let request = MonthLoad {
            generation,
            month: current,
        };
        spawn(async move {
            load_month(request, state).await;
        });
    });

    use_effect(move || {
        let mut state = state;
        let _ = (state.day_reload)();
        let Some(date) = (state.selected)() else {
            return;
        };
        let generation = state.day_generation.peek().wrapping_add(1);
        state.day_generation.set(generation);
        state.day_error.set(None);
        let request = DayLoad { generation, date };
        spawn(async move {
            load_day(request, state).await;
        });
    });

    state
}

/// カレンダー画面の操作。
#[derive(Clone, Copy)]
struct CalendarHandlers {
    select: EventHandler<JstDate>,
    previous: EventHandler<()>,
    next: EventHandler<()>,
    month_retry: EventHandler<()>,
    day_retry: EventHandler<()>,
}

fn calendar_handlers(state: CalendarSignals) -> CalendarHandlers {
    CalendarHandlers {
        select: EventHandler::new(move |date: JstDate| {
            let mut state = state;
            state.selected.set(Some(date));
            state.day_data.set(None);
            state.day_error.set(None);
        }),
        previous: EventHandler::new(move |_: ()| move_to_month(state, previous_month)),
        next: EventHandler::new(move |_: ()| move_to_month(state, next_month)),
        month_retry: EventHandler::new(move |_: ()| {
            let mut state = state;
            state.month_reload += 1;
        }),
        day_retry: EventHandler::new(move |_: ()| {
            let mut state = state;
            state.day_reload += 1;
        }),
    }
}

/// 月を移し、前の月の読み込み結果と選択を捨てる。
fn move_to_month(state: CalendarSignals, target: impl FnOnce(JstMonth) -> Option<JstMonth>) {
    let mut state = state;
    let Some(month) = (state.month)() else {
        return;
    };
    let Some(month) = target(month) else {
        return;
    };
    state.month.set(Some(month));
    state.month_data.set(None);
    state.selected.set(None);
    state.day_data.set(None);
    state.day_error.set(None);
}

/// 月の見出しと、前後の月への移動。
#[component]
fn MonthHeading(
    month: Option<JstMonth>,
    training_days: u32,
    on_previous: EventHandler<()>,
    on_next: EventHandler<()>,
) -> Element {
    let (year, month_number) = month
        .map(|current| (current.start().year(), current.start().month()))
        .unwrap_or_default();
    rsx! {
        div { class: "flex h-11 items-center justify-between",
            button {
                class: "flex h-11 w-11 items-center justify-center border border-line",
                "data-calendar-previous": "true",
                "aria-label": text(Key::CalendarPreviousMonth),
                onclick: move |_| on_previous.call(()),
                svg {
                    class: "h-5 w-5",
                    view_box: "0 0 24 24",
                    fill: "none",
                    stroke: "currentColor",
                    stroke_width: "2",
                    stroke_linecap: "round",
                    stroke_linejoin: "round",
                    path { d: "M15 5l-7 7 7 7" }
                }
            }
            div { class: "flex items-baseline gap-3",
                span { class: "text-sm font-bold",
                    span { class: "font-condensed text-[26px] font-semibold", "{year}" }
                    " 年 "
                    span { class: "font-condensed text-[26px] font-semibold", "{month_number}" }
                    " 月"
                }
                span { class: "text-xs text-text-sub",
                    "{text(Key::CalendarTrainingDays)} "
                    strong {
                        class: "font-condensed text-xl font-semibold text-text",
                        "data-calendar-training-days": "{training_days}",
                        "{training_days}"
                    }
                    " {text(Key::CalendarDaysUnit)}"
                }
            }
            button {
                class: "flex h-11 w-11 items-center justify-center border border-line",
                "data-calendar-next": "true",
                "aria-label": text(Key::CalendarNextMonth),
                onclick: move |_| on_next.call(()),
                svg {
                    class: "h-5 w-5",
                    view_box: "0 0 24 24",
                    fill: "none",
                    stroke: "currentColor",
                    stroke_width: "2",
                    stroke_linecap: "round",
                    stroke_linejoin: "round",
                    path { d: "M9 5l7 7-7 7" }
                }
            }
        }
    }
}

/// 曜日の見出しと、月の日のセルのグリッド。
#[component]
fn MonthGrid(
    month: Option<JstMonth>,
    view: Option<CalendarMonth>,
    selected: Option<JstDate>,
    today: Option<JstDate>,
    on_select: EventHandler<JstDate>,
) -> Element {
    let cells = build_cells(month, view.as_ref(), selected, today);
    rsx! {
        div {
            class: "mt-3 grid grid-cols-7 text-center text-[11px] leading-5 text-text-sub",
            "aria-hidden": "true",
            for weekday in weekdays() {
                span { "{weekday}" }
            }
        }
        div { class: "grid grid-cols-7",
            for cell in cells {
                if let Some(cell) = cell {
                    CalendarDayCell { cell: cell, on_select: on_select }
                } else {
                    div { class: "h-[50px] border-t border-line", "aria-hidden": "true" }
                }
            }
        }
    }
}

/// 月の読み込み中と、読み込みに失敗したときの表示。
#[component]
fn MonthStatus(loading: bool, error: Option<String>, on_retry: EventHandler<()>) -> Element {
    rsx! {
        if loading {
            p { class: "mt-2 text-[13px] text-text-sub", "{text(Key::CalendarLoading)}" }
        }
        if let Some(message) = error {
            p { class: "mt-2 text-[13px] text-primary", "{message}" }
            button {
                class: "mt-1 min-h-11 border border-line px-3 text-[13px]",
                onclick: move |_| on_retry.call(()),
                "{text(Key::CalendarRetry)}"
            }
        }
    }
}

#[component]
fn PartLegend(counts: Vec<(BodyPart, u32)>) -> Element {
    rsx! {
        div {
            class: "mt-2 flex flex-wrap items-center gap-x-3 gap-y-0.5 text-[11px] text-text-sub",
            "data-calendar-legend": "true",
            "aria-label": text(Key::CalendarParts),
            for (part, days) in counts {
                span {
                    class: "flex items-center gap-1",
                    "data-calendar-part": part.as_str(),
                    "data-calendar-part-days": "{days}",
                    span {
                        class: format!("h-1.5 w-[14px] {}", part_color_class(part)),
                        "data-calendar-swatch": part.as_str(),
                        "aria-hidden": "true"
                    }
                    span { "{part_label(part)}" }
                    span { class: "font-condensed text-[13px] font-semibold text-text",
                        "{days} {text(Key::CalendarDaysUnit)}"
                    }
                }
            }
        }
    }
}

#[component]
fn CalendarDayCell(cell: CalendarCell, on_select: EventHandler<JstDate>) -> Element {
    let class = day_cell_class(&cell);
    let number_class = day_number_class(&cell);
    let date = cell.date;
    let label = day_aria_label(&cell);
    let parts = cell
        .body_parts
        .iter()
        .map(|part| part.as_str())
        .collect::<Vec<_>>()
        .join(",");
    rsx! {
        button {
            class: class,
            "data-calendar-day": "{date}",
            "data-calendar-parts": parts,
            "data-calendar-selected": if cell.selected { "true" } else { "false" },
            "data-calendar-future": if cell.future { "true" } else { "false" },
            "aria-pressed": if cell.selected { "true" } else { "false" },
            "aria-label": label,
            onclick: move |_| on_select.call(date),
            span { class: number_class, "{date.day()}" }
            if !cell.body_parts.is_empty() {
                span { class: "flex h-1.5 w-[26px] gap-0.5", "aria-hidden": "true",
                    for part in cell.body_parts.iter() {
                        span {
                            class: format!("flex-1 {}", part_color_class(*part)),
                            "data-calendar-marker": part.as_str()
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn DayDetail(
    detail: Option<CalendarDayDetail>,
    selected: Option<JstDate>,
    error: Option<String>,
    loading: bool,
    on_retry: EventHandler<()>,
) -> Element {
    let date_attr = selected.map(|date| date.to_string()).unwrap_or_default();
    rsx! {
        section {
            class: "mt-1 border-t border-text pt-2.5",
            "data-calendar-detail": date_attr,
            if let Some(message) = error {
                p { class: "text-[13px] text-primary", "{message}" }
                button {
                    class: "mt-1 min-h-11 border border-line px-3 text-[13px]",
                    onclick: move |_| on_retry.call(()),
                    "{text(Key::CalendarRetry)}"
                }
            } else if let Some(detail) = detail {
                DayDetailBody { detail: detail }
            } else if loading {
                p { class: "text-[13px] text-text-sub", "{text(Key::CalendarLoading)}" }
            } else {
                p { class: "text-[13px] text-text-sub", "{text(Key::CalendarSelectDay)}" }
            }
        }
    }
}

#[component]
fn DayDetailBody(detail: CalendarDayDetail) -> Element {
    let parts = day_parts(&detail.trainings);
    let total = detail.total_weight_kg;
    rsx! {
        div { class: "flex items-baseline justify-between gap-2",
            h2 { class: "flex items-center gap-2 text-sm font-bold",
                "{day_title(detail.date)}"
                for part in parts {
                    span {
                        class: format!("h-2.5 w-2.5 {}", part_color_class(part)),
                        "data-calendar-day-part": part.as_str(),
                        "aria-hidden": "true"
                    }
                }
            }
            span { class: "text-[11px] text-text-sub",
                "{text(Key::CalendarTotalWeight)} "
                strong {
                    class: "font-condensed text-[18px] font-semibold text-text",
                    "data-calendar-day-total": "{total}",
                    "{format_weight(total)}"
                }
            }
        }
        if detail.trainings.is_empty() {
            p { class: "mt-2 text-[13px] text-text-sub", "{text(Key::CalendarNoTraining)}" }
        }
        for training in detail.trainings {
            div { "data-calendar-training": training.training_id.clone(),
                for exercise in training.exercises {
                    ExerciseDetail { exercise: exercise }
                }
            }
        }
    }
}

#[component]
fn ExerciseDetail(exercise: CalendarExercise) -> Element {
    let part = exercise.body_part;
    let total = exercise.total_weight_kg;
    let name = exercise.name.clone();
    let exercise_id = exercise.exercise_id.clone();
    rsx! {
        div {
            class: "flex flex-col border-b border-line py-1",
            "data-calendar-exercise": exercise_id,
            "data-calendar-exercise-name": name.clone(),
            "data-calendar-exercise-total": "{total}",
            div { class: "flex min-h-6 items-center justify-between gap-2",
                div { class: "flex min-w-0 items-center gap-2",
                    span {
                        class: format!("h-2.5 w-2.5 shrink-0 {}", part_color_class(part)),
                        "data-calendar-exercise-part": part.as_str(),
                        "aria-hidden": "true"
                    }
                    span { class: "truncate text-[13px]", "{name}" }
                }
                span { class: "shrink-0 font-condensed text-[15px] font-semibold",
                    "{format_weight(total)}"
                }
            }
            ol { class: "mt-1 flex flex-col gap-1",
                for (index, set) in exercise.sets.iter().enumerate() {
                    SetDetail { index: index + 1, set: set.clone() }
                }
            }
        }
    }
}

#[component]
fn SetDetail(index: usize, set: CalendarSet) -> Element {
    let body = set_text(&set);
    let notes = set_notes(&set);
    let kind = set.kind.as_str();
    let total = set.total_weight_kg;
    let notes_text = notes.join(" / ");
    rsx! {
        li {
            class: "flex flex-col gap-0.5 pl-4",
            "data-calendar-set": "{index}",
            "data-calendar-set-kind": kind,
            "data-calendar-set-total": "{total}",
            "data-calendar-set-text": body.clone(),
            "data-calendar-set-notes": notes_text.clone(),
            span { class: "font-condensed text-[13px] font-semibold", "{body}" }
            span { class: "text-[11px] text-text-sub", "{notes_text}" }
        }
    }
}

/// 日付を指定した追加の操作の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AddRecordError {
    /// 日付が存在しない、または形式が違う。
    InvalidDate,
    /// 所要時間が 1 分以上 1440 分以下でない。
    InvalidDuration,
    /// 端末に保存できない。
    Storage,
}

/// 日付を指定して過去の記録を追加する入口。
///
/// 追加する記録は開始時刻を持たず、指定した日付と所要時間を持つ。サーバーが受理した後に
/// 月と日を読み直すため、追加した日付のトレーニングがカレンダーに現れる。
#[component]
fn CalendarAddForm(state: CalendarSignals, today: Option<JstDate>) -> Element {
    let initial_date = (state.selected)()
        .or(today)
        .map(|date| date.to_string())
        .unwrap_or_default();
    let mut date = use_signal(|| initial_date);
    let mut minutes = use_signal(String::new);
    let error = use_signal(|| Option::<String>::None);
    let busy = use_signal(|| false);
    let added = use_signal(|| Option::<String>::None);
    let busy_now = busy();
    let error_now = error();
    let date_value = date();
    let minutes_value = minutes();

    rsx! {
        section {
            class: "mt-2 flex flex-col gap-2 border border-line bg-surface p-2",
            "data-calendar-add": "true",
            h2 { class: "text-sm font-bold", "{text(Key::CalendarAddTitle)}" }
            label { class: "flex flex-col gap-1 text-[11px]",
                "{text(Key::CalendarAddDateLabel)}"
                DateSelect {
                    value: date_value.clone(),
                    min_year: 2000,
                    max_year: 2100,
                    name: "calendar-add-date",
                    on_change: move |value| date.set(value),
                }
            }
            label { class: "flex flex-col gap-1 text-[11px]",
                "{text(Key::CalendarAddDurationLabel)}"
                input {
                    class: "min-h-11 border border-line bg-surface px-2 text-[13px]",
                    r#type: "number",
                    min: "1",
                    max: "1440",
                    "data-calendar-add-duration": "true",
                    value: "{minutes_value}",
                    oninput: move |event| minutes.set(event.value()),
                }
            }
            if let Some(message) = error_now {
                p { class: "text-[11px] text-primary", "data-calendar-add-error": "true", "{message}" }
            }
            button {
                class: "min-h-11 border border-primary bg-primary px-3 text-[13px] font-bold text-on-primary",
                "data-calendar-add-submit": "true",
                disabled: busy_now,
                onclick: move |_| {
                    let date_text = date();
                    let minutes_text = minutes();
                    spawn(add_record(
                        date_text,
                        minutes_text,
                        state,
                        error,
                        busy,
                        added,
                    ));
                },
                "{text(Key::CalendarAddSubmit)}"
            }
            if let Some(training_id) = (added)() {
                Link {
                    to: Route::RecordSession { training_id },
                    class: "flex min-h-11 items-center justify-center border border-primary px-3 text-[13px] text-primary",
                    "data-calendar-add-open-record": "true",
                    "{text(Key::CalendarAddOpenRecord)}"
                }
            }
        }
    }
}

/// 日付と所要時間から過去の記録を作り、端末の保存と送信待ちの列を通す。
async fn add_record(
    date_text: String,
    minutes_text: String,
    mut state: CalendarSignals,
    mut error: Signal<Option<String>>,
    mut busy: Signal<bool>,
    mut added: Signal<Option<String>>,
) {
    busy.set(true);
    let result = save_backfill(&date_text, &minutes_text).await;
    busy.set(false);
    match result {
        Ok(session) => {
            error.set(None);
            added.set(Some(session.id.clone()));
            if let Some(date) = JstDate::parse(&session.date) {
                state.month.set(Some(date.month_range()));
                state.selected.set(Some(date));
            }
            state.month_reload += 1;
            state.day_reload += 1;
        }
        Err(AddRecordError::InvalidDate) => {
            error.set(Some(text(Key::CalendarAddInvalidDate).to_owned()));
        }
        Err(AddRecordError::InvalidDuration) => {
            error.set(Some(text(Key::CalendarAddInvalidDuration).to_owned()));
        }
        Err(AddRecordError::Storage) => {
            error.set(Some(text(Key::CalendarAddFailed).to_owned()));
        }
    }
}

/// 追加する記録を端末に保存し、サーバーへの送信を待つ。
async fn save_backfill(
    date_text: &str,
    minutes_text: &str,
) -> Result<warrior_core::sync::SessionRecord, AddRecordError> {
    let Some(date) = JstDate::parse(date_text.trim()) else {
        return Err(AddRecordError::InvalidDate);
    };
    let minutes = minutes_text
        .trim()
        .parse::<i64>()
        .map_err(|_| AddRecordError::InvalidDuration)?;
    let duration_seconds = minutes
        .checked_mul(60)
        .ok_or(AddRecordError::InvalidDuration)?;
    let now = now_iso();
    let session = backfill_session(
        new_uuid().map_err(|_| AddRecordError::Storage)?,
        date.to_string(),
        duration_seconds,
        now,
    );
    let operation = backfill_operation(&session).map_err(|_| AddRecordError::InvalidDuration)?;
    let storage = storage::current()
        .await
        .map_err(|_| AddRecordError::Storage)?;
    storage
        .save_session_backfill(&session, &operation)
        .await
        .map_err(|_| AddRecordError::Storage)?;
    sync::flush().await;
    Ok(session)
}

/// 月の読み込み要求。応答を適用してよいかを、要求した月と世代で決める。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MonthLoad {
    /// 要求の世代。要求を出すたびに進む。
    generation: u32,
    month: JstMonth,
}

impl MonthLoad {
    /// いまの世代が要求のままで、表示中の月も要求のままなら応答を適用する。
    fn is_current(self, generation: u32, month: Option<JstMonth>) -> bool {
        self.generation == generation && month == Some(self.month)
    }
}

/// 日の読み込み要求。応答を適用してよいかを、要求した日と世代で決める。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DayLoad {
    /// 要求の世代。要求を出すたびに進む。
    generation: u32,
    date: JstDate,
}

impl DayLoad {
    /// いまの世代が要求のままで、選択中の日も要求のままなら応答を適用する。
    fn is_current(self, generation: u32, selected: Option<JstDate>) -> bool {
        self.generation == generation && selected == Some(self.date)
    }
}

async fn load_month(request: MonthLoad, mut state: CalendarSignals) {
    let result = calendar_api::month(request.month.start()).await;
    if !request.is_current(*state.month_generation.peek(), *state.month.peek()) {
        return;
    }
    match result {
        Ok(view) => {
            state.month_data.set(Some(view));
            state.month_error.set(None);
        }
        Err(_) => {
            state.month_data.set(None);
            state
                .month_error
                .set(Some(text(Key::CalendarLoadFailed).to_owned()));
        }
    }
}

async fn load_day(request: DayLoad, mut state: CalendarSignals) {
    let result = calendar_api::day(request.date).await;
    if !request.is_current(*state.day_generation.peek(), *state.selected.peek()) {
        return;
    }
    match result {
        Ok(detail) => {
            state.day_data.set(Some(detail));
            state.day_error.set(None);
        }
        Err(_) => {
            state.day_data.set(None);
            state
                .day_error
                .set(Some(text(Key::CalendarDayLoadFailed).to_owned()));
        }
    }
}

fn current_date() -> Option<JstDate> {
    let seconds = (js_sys::Date::now() / 1000.0) as i64;
    UtcInstant::from_epoch_seconds(seconds).jst_date()
}

fn month_text(month: JstMonth) -> String {
    format!("{:04}-{:02}", month.start().year(), month.start().month())
}

#[cfg(test)]
mod tests {
    //! 月と日の読み込み結果を適用してよいかの単体テスト。
    //!
    //! 要求の受入基準は E2E が担う。応答の到着順は実ブラウザの通信に依存するため、
    //! ここでは古い応答を捨てる条件を直接確かめる。

    use warrior_core::calendar::next_month;
    use warrior_core::jst::JstDate;
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::{DayLoad, MonthLoad};

    #[wasm_bindgen_test]
    fn a_month_response_of_a_month_that_moved_away_is_not_applied() {
        let november = JstDate::new(2026, 11, 12).unwrap().month_range();
        let december = next_month(november).expect("a next month");
        let request = MonthLoad {
            generation: 2,
            month: november,
        };
        assert!(request.is_current(2, Some(november)));
        assert!(
            !request.is_current(2, Some(december)),
            "the displayed month moved before the response arrived"
        );
        assert!(
            !request.is_current(3, Some(november)),
            "a newer request replaced this request"
        );
    }

    #[wasm_bindgen_test]
    fn a_day_response_of_an_unselected_day_is_not_applied() {
        let first = JstDate::new(2026, 11, 1).unwrap();
        let second = JstDate::new(2026, 11, 2).unwrap();
        let request = DayLoad {
            generation: 5,
            date: first,
        };
        assert!(request.is_current(5, Some(first)));
        assert!(
            !request.is_current(5, Some(second)),
            "the selected day changed before the response arrived"
        );
        assert!(
            !request.is_current(5, None),
            "the month moved and the day is no longer selected"
        );
        assert!(
            !request.is_current(6, Some(first)),
            "a newer request replaced this request"
        );
    }
}
