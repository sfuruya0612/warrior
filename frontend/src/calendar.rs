//! カレンダーの月の集計と日の詳細の API。

use warrior_core::calendar::{CalendarDayDetail, CalendarMonth};
use warrior_core::jst::JstDate;

use crate::http::{self, HttpError};

/// 月の日ごとの部位と日数を返す。
pub async fn month(month: JstDate) -> Result<CalendarMonth, HttpError> {
    let text = format!("{:04}-{:02}", month.year(), month.month());
    http::request_json("GET", &format!("/api/calendar?month={text}")).await
}

/// その日のトレーニング、種目、セット、総重量を返す。
pub async fn day(date: JstDate) -> Result<CalendarDayDetail, HttpError> {
    http::request_json("GET", &format!("/api/calendar/{date}")).await
}
