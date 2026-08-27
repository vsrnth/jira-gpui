use chrono::{Local, LocalResult, TimeZone};
use time::{Date, OffsetDateTime, UtcOffset};

pub(crate) fn format_timestamp(value: OffsetDateTime) -> String {
    let offset = local_offset_for(value).unwrap_or(UtcOffset::UTC);
    format_timestamp_with_offset(value, offset)
}

/// Formats a timestamp with an injected offset for deterministic fixture views.
/// `None` preserves the production local-time behavior.
pub(crate) fn format_timestamp_for(value: OffsetDateTime, offset: Option<UtcOffset>) -> String {
    offset.map_or_else(
        || format_timestamp(value),
        |offset| format_timestamp_with_offset(value, offset),
    )
}

/// Resolve the system offset for this instant, rather than using the offset at the current time.
/// This keeps historical/future timestamps correct across daylight-saving transitions. If the
/// platform cannot resolve its local timezone, callers use UTC as a safe, explicit fallback.
fn local_offset_for(value: OffsetDateTime) -> Option<UtcOffset> {
    let local = match Local.timestamp_opt(value.unix_timestamp(), value.nanosecond()) {
        LocalResult::Single(local) => local,
        LocalResult::None | LocalResult::Ambiguous(_, _) => return None,
    };
    UtcOffset::from_whole_seconds(local.offset().local_minus_utc()).ok()
}

pub(super) fn format_timestamp_with_offset(value: OffsetDateTime, offset: UtcOffset) -> String {
    let local = value.to_offset(offset);
    format!(
        "{} {:02}, {} · {:02}:{:02} {}",
        month_name(local.month() as u8),
        local.day(),
        local.year(),
        local.hour(),
        local.minute(),
        format_offset(offset)
    )
}

fn format_offset(offset: UtcOffset) -> String {
    let seconds = offset.whole_seconds();
    if seconds == 0 {
        return "UTC".to_owned();
    }
    let sign = if seconds.is_negative() { '-' } else { '+' };
    let absolute = seconds.unsigned_abs();
    format!("{sign}{:02}:{:02}", absolute / 3600, (absolute % 3600) / 60)
}

pub(super) fn format_date(value: Date) -> String {
    format!(
        "{} {:02}, {}",
        month_name(value.month() as u8),
        value.day(),
        value.year()
    )
}

pub(super) fn format_bytes(value: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let value_f64 = value as f64;
    if value_f64 >= GIB {
        format!("{:.1} GiB", value_f64 / GIB)
    } else if value_f64 >= MIB {
        format!("{:.1} MiB", value_f64 / MIB)
    } else if value_f64 >= KIB {
        format!("{:.1} KiB", value_f64 / KIB)
    } else {
        format!("{value} B")
    }
}

fn month_name(month: u8) -> &'static str {
    match month {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        _ => "Dec",
    }
}
