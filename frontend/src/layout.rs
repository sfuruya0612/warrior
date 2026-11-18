//! 共通の画面の枠、下部のタブ、記録中のレールとレストタイマーの枠。

use dioxus::prelude::*;
use dioxus_router::{router, Link, Outlet};
use warrior_core::jst::Weekday;

use crate::record_rail::RecordRailState;
use crate::routes::{navigate, tab_is_active, tab_key, tab_route, Route, Tab, TABS};
use crate::screens::record::{RecordWakeLock, RestTimerBar};
use crate::text::{text, Key};

/// 全ての画面を包む共通の枠。
#[component]
pub fn AppShell() -> Element {
    let authenticated = use_signal(|| false);
    let auth_checked = authenticated;
    use_effect(move || {
        let mut authenticated = auth_checked;
        spawn(async move {
            if crate::http::request_json::<serde_json::Value>("GET", "/api/me")
                .await
                .is_err()
            {
                navigate("/login");
                return;
            }
            authenticated.set(true);
        });
    });
    if !authenticated() {
        return rsx! {
            div {
                class: "flex h-dvh w-full items-center justify-center bg-bg text-text",
                "data-auth-check": "true",
            }
        };
    }
    let current = router().current::<Route>();
    let route_name = crate::routes::route_name(&current);
    let record = matches!(current, Route::Record {} | Route::RecordSession { .. });
    let record_training_id = match &current {
        Route::RecordSession { training_id } => Some(training_id.clone()),
        _ => None,
    };

    rsx! {
        div {
            class: "mx-auto flex h-dvh w-full max-w-[390px] flex-col overflow-hidden bg-bg text-text",
            "data-route": route_name,
            div { class: "shrink-0 pt-[54px]" }
            header { class: "flex h-8 shrink-0 items-center justify-between border-b border-line px-5",
                // ロゴをヘッダーの高さに収めつつ、リンクの当たり判定は 44px 以上にする。
                // 上にはみ出した分は端末の表示領域を空ける余白に重なる。
                Link {
                    to: Route::Home {},
                    class: "flex h-11 shrink-0 -mt-3 items-center gap-[10px] self-start pt-3",
                    svg {
                        class: "h-[23px] w-[30px]",
                        view_box: "40 70 440 340",
                        "aria-hidden": "true",
                        polygon {
                            points: "58,116 134,116 184,290 218,212 294,212 328,290 378,116 454,116 374,396 298,396 256,299 214,396 138,396",
                            fill: "currentColor",
                        }
                        polygon {
                            points: "298,396 374,396 463,84 387,84",
                            class: "text-primary",
                            fill: "currentColor",
                            stroke: "var(--bg)",
                            stroke_width: "10",
                            stroke_linejoin: "miter",
                        }
                    }
                    span { class: "font-condensed text-[22px] font-bold leading-none tracking-[0.16em]", "{text(Key::AppName)}" }
                }
                span { class: "text-[13px] text-text-sub", "{today_label()}" }
            }
            if record {
                div { class: "flex min-h-0 flex-1",
                    aside {
                        class: "flex w-[60px] shrink-0 flex-col items-stretch gap-1 overflow-y-auto border-r border-line bg-surface pt-3 text-[11px] text-text-sub",
                        "data-exercise-rail": "true",
                        RecordRail { training_id: record_training_id.clone() }
                    }
                    main { class: "min-h-0 min-w-0 flex-1 overflow-y-auto", Outlet::<Route> {} }
                }
                div {
                    class: "flex h-[72px] shrink-0 items-center justify-center border-t border-line bg-surface text-[11px] text-text-sub",
                    RestTimerBar { training_id: record_training_id.clone() }
                }
                RecordWakeLock { training_id: record_training_id }
            } else {
                main { class: "min-h-0 flex-1 overflow-y-auto", Outlet::<Route> {} }
                TabBar { current }
            }
        }
    }
}

#[component]
fn RecordRail(training_id: Option<String>) -> Element {
    let mut rail = use_context::<Signal<RecordRailState>>();
    let state = (rail)();
    let shows = training_id
        .as_deref()
        .is_some_and(|id| state.training_id.as_deref() == Some(id));
    rsx! {
        span { class: "shrink-0 text-center font-bold", "{text(Key::RecordRail)}" }
        if shows {
            for exercise in state.exercises {
                {
                    let selected = state.selected.as_deref() == Some(exercise.id.as_str());
                    let class = if selected {
                        "w-full shrink-0 border-b border-primary px-1 py-2 text-center text-[10px] font-bold leading-4 text-primary"
                    } else {
                        "w-full shrink-0 border-b border-line px-1 py-2 text-center text-[10px] leading-4 text-text-sub"
                    };
                    rsx! {
                        button {
                            class,
                            "data-exercise-row": "true",
                            "data-training-exercise-id": "{exercise.id}",
                            "data-exercise-id": "{exercise.exercise_id}",
                            "data-selected": if selected { "true" } else { "false" },
                            onclick: {
                                let id = exercise.id.clone();
                                move |_| {
                                    rail.write().selected = Some(id.clone());
                                }
                            },
                            span { class: "w-full break-words", "{exercise.name}" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn TabBar(current: Route) -> Element {
    rsx! {
        nav { class: "flex shrink-0 border-t border-line bg-bg pb-6",
            for tab in TABS {
                {
                    let active = tab_is_active(&current, tab);
                    let class = if active {
                        "flex h-14 flex-1 flex-col items-center justify-center gap-[3px] text-[11px] font-bold text-primary"
                    } else {
                        "flex h-14 flex-1 flex-col items-center justify-center gap-[3px] text-[11px] font-medium text-text-sub"
                    };
                    rsx! {
                        Link { to: tab_route(tab), class,
                            svg {
                                class: "h-[22px] w-[22px]",
                                view_box: "0 0 24 24",
                                fill: "none",
                                stroke: "currentColor",
                                stroke_width: "1.8",
                                stroke_linecap: "round",
                                stroke_linejoin: "round",
                                path { d: tab_icon(tab) }
                            }
                            span { "{text(tab_key(tab))}" }
                        }
                    }
                }
            }
        }
    }
}

fn tab_icon(tab: Tab) -> &'static str {
    match tab {
        Tab::Home => "M3 11l9-7 9 7v9a1 1 0 0 1-1 1h-5v-6H9v6H4a1 1 0 0 1-1-1z",
        Tab::Calendar => "M3 5h18v16H3zM3 10h18M8 3v4M16 3v4",
        Tab::Library => "M6.5 6.5v11M3.5 9v6M17.5 6.5v11M20.5 9v6M6.5 12h11",
        Tab::Analysis => "M5 20V11M12 20V4M19 20v-6M2 20h20",
        Tab::MyPage => "M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM4 21c0-4 3.6-7 8-7s8 3 8 7",
    }
}

fn today_label() -> String {
    let Some(today) = crate::profile::today() else {
        return String::new();
    };
    let weekday = match today.weekday() {
        Weekday::Monday => "月",
        Weekday::Tuesday => "火",
        Weekday::Wednesday => "水",
        Weekday::Thursday => "木",
        Weekday::Friday => "金",
        Weekday::Saturday => "土",
        Weekday::Sunday => "日",
    };
    format!("{}月{}日({weekday})", today.month(), today.day())
}
