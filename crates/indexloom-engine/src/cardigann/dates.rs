//! Tracker date layouts (.NET and Go), absolute and relative dates, normalized to UTC.
use super::{EngineError, Regex, transform::error};
use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub(super) fn filter(
    name: &str,
    input: &str,
    args: &[String],
    now: OffsetDateTime,
) -> Result<String, EngineError> {
    if name == "fuzzytime"
        && args
            .first()
            .is_some_and(|v| !["UK", "US", "USA"].contains(&v.as_str()))
    {
        return Err(error(name, "unsupported fuzzy date region"));
    }
    let timestamp = match name {
        "dateparse" | "timeparse" => layout(input, &args[0], now),
        "timeago" | "reltime" => relative(input, now),
        "fuzzytime" => fuzzy(input, args.first().map(String::as_str), now),
        _ => None,
    }
    .ok_or_else(|| error(name, "date or layout could not be parsed"))?;
    timestamp
        .format(&Rfc3339)
        .map_err(|_| error(name, "date outside supported range"))
}
fn from_unix(seconds: i64) -> Option<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp(seconds).ok()
}
fn utc_now(now: OffsetDateTime) -> DateTime<Utc> {
    Utc.timestamp_opt(now.unix_timestamp(), 0)
        .single()
        .expect("time and chrono share the supported range")
}

pub(super) fn layout(input: &str, format: &str, now: OffsetDateTime) -> Option<OffsetDateTime> {
    if format.trim().is_empty() {
        return None;
    }
    let canonical;
    let input = if format.contains("Z07:00") && input.ends_with('Z') {
        canonical = format!("{}+00:00", input.strip_suffix('Z')?);
        canonical.as_str()
    } else {
        input
    };
    let format = translate(format)?;
    let input = input.split_whitespace().collect::<Vec<_>>().join(" ");
    if format.contains("%Z")
        && !["UTC", "GMT", "UT", "Z"]
            .iter()
            .any(|zone| input.ends_with(zone))
    {
        return None;
    }
    let mut parsed = chrono::format::Parsed::new();
    chrono::format::parse(
        &mut parsed,
        &input,
        chrono::format::StrftimeItems::new(&format),
    )
    .ok()?;
    let missing_year = parsed.year.is_none() && parsed.year_mod_100.is_none();
    let current = utc_now(now);
    if missing_year {
        parsed.set_year(i64::from(current.year())).ok()?;
    }
    if parsed.month.is_none() {
        parsed.set_month(i64::from(current.month())).ok()?;
    }
    if parsed.day.is_none() {
        parsed.set_day(i64::from(current.day())).ok()?;
    }
    if parsed.hour_div_12.is_none() && parsed.hour_mod_12.is_none() {
        parsed.set_hour(0).ok()?;
    }
    if parsed.minute.is_none() {
        parsed.set_minute(0).ok()?;
    }
    if parsed.second.is_none() {
        parsed.set_second(0).ok()?;
    }
    let mut date = if parsed.offset.is_some() {
        parsed.to_datetime().ok()?.with_timezone(&Utc)
    } else {
        parsed.to_naive_datetime_with_offset(0).ok()?.and_utc()
    };
    // Missing-year dates refer to the latest occurrence, as in Cardigann.
    if missing_year && date > current {
        date = date.with_year(current.year() - 1)?;
    }
    from_unix(date.timestamp())
}

fn translate(source: &str) -> Option<String> {
    // Tokenize once, longest token first. Repeated replacement corrupts overlapping Go tokens.
    let dotnet = source.contains('y')
        || source.contains("HH")
        || source.contains("dd")
        || source.contains("tt");
    let net = [
        ("yyyy", "%Y"),
        ("MMMM", "%B"),
        ("dddd", "%A"),
        ("MMM", "%b"),
        ("ddd", "%a"),
        ("zzz", "%:z"),
        ("yy", "%y"),
        ("MM", "%m"),
        ("dd", "%d"),
        ("HH", "%H"),
        ("hh", "%I"),
        ("mm", "%M"),
        ("ss", "%S"),
        ("tt", "%p"),
        ("zz", "%z"),
        ("M", "%m"),
        ("d", "%d"),
        ("H", "%H"),
        ("h", "%I"),
        ("m", "%M"),
        ("s", "%S"),
        ("z", "%z"),
    ];
    let go = [
        ("January", "%B"),
        ("Monday", "%A"),
        ("Z07:00", "%:z"),
        ("-07:00", "%:z"),
        ("-0700", "%z"),
        ("Z0700", "%z"),
        ("2006", "%Y"),
        ("MST", "%Z"),
        ("Jan", "%b"),
        ("Mon", "%a"),
        ("_2", "%e"),
        ("06", "%y"),
        ("01", "%m"),
        ("02", "%d"),
        ("15", "%H"),
        ("03", "%I"),
        ("04", "%M"),
        ("05", "%S"),
        ("PM", "%p"),
        ("pm", "%p"),
        ("1", "%m"),
        ("2", "%d"),
        ("3", "%I"),
        ("4", "%M"),
        ("5", "%S"),
    ];
    let mut rest = source;
    let mut output = String::new();
    let mut quoted = false;
    while !rest.is_empty() {
        if let Some(tail) = rest.strip_prefix('\'') {
            quoted = !quoted;
            rest = tail;
            continue;
        }
        if !quoted {
            let token = if dotnet {
                net.iter().find(|(a, _)| rest.starts_with(a))
            } else {
                go.iter().find(|(a, _)| rest.starts_with(a))
            };
            if let Some((a, b)) = token {
                output.push_str(b);
                rest = &rest[a.len()..];
                continue;
            }
            if rest.starts_with('f')
                || rest.starts_with('F')
                || rest.starts_with("000")
                || rest.starts_with("999")
            {
                let c = rest.chars().next()?;
                let len = rest.chars().take_while(|v| *v == c).count();
                output.push_str("%f");
                rest = &rest[len..];
                continue;
            }
        }
        let c = rest.chars().next()?;
        if c == '%' {
            output.push('%');
        }
        output.push(c);
        rest = &rest[c.len_utf8()..];
    }
    (!quoted).then_some(output)
}

pub(super) fn relative(input: &str, now: OffsetDateTime) -> Option<OffsetDateTime> {
    let input = input.to_lowercase();
    if ["now", "just now", "right now"].contains(&input.trim()) {
        return Some(now);
    }
    let regex = Regex::new(r"(?i)([0-9]+(?:\.[0-9]+)?)\s*([a-z]+)").ok()?;
    let remainder = regex.replace_all(&input, "");
    if remainder
        .split(|c: char| c.is_whitespace() || c == ',')
        .any(|v| !["", "ago", "and"].contains(&v))
    {
        return None;
    }
    let mut seconds = 0.0;
    let mut count = 0;
    for cap in regex.captures_iter(&input) {
        let number = cap[1].parse::<f64>().ok()?;
        let unit = &cap[2];
        let multiplier = match unit {
            "s" | "sec" | "secs" | "second" | "seconds" => 1.0,
            "m" | "min" | "mins" | "minute" | "minutes" => 60.0,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600.0,
            "d" | "day" | "days" => 86400.0,
            "w" | "wk" | "wks" | "week" | "weeks" => 604_800.0,
            "mo" | "month" | "months" => 2_592_000.0,
            "y" | "yr" | "yrs" | "year" | "years" => 31_536_000.0,
            _ => return None,
        };
        seconds += number * multiplier;
        count += 1;
    }
    if count == 0 || !seconds.is_finite() || seconds > 253_402_300_799.0 {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)]
    now.checked_sub(time::Duration::seconds(seconds as i64))
}

pub(super) fn fuzzy(
    input: &str,
    region: Option<&str>,
    now: OffsetDateTime,
) -> Option<OffsetDateTime> {
    if let Some(date) = super::filters::parse_date(input) {
        return Some(date);
    }
    if let Ok(epoch) = input.trim().parse::<i64>() {
        return from_unix(epoch);
    }
    let lower = input.to_lowercase();
    if lower.contains("ago") || lower.contains("now") {
        return relative(input, now);
    }
    let current = utc_now(now);
    for (word, days) in [("today", 0), ("yesterday", -1), ("tomorrow", 1)] {
        if let Some(rest) = lower.strip_prefix(word) {
            let rest = rest
                .trim_start_matches([',', ' '])
                .strip_prefix("at ")
                .unwrap_or(rest.trim_start_matches([',', ' ']));
            let clock = if rest.is_empty() {
                NaiveTime::from_hms_opt(0, 0, 0)?
            } else {
                parse_clock(rest)?
            };
            return from_unix(
                (current.date_naive() + chrono::Duration::days(days))
                    .and_time(clock)
                    .and_utc()
                    .timestamp(),
            );
        }
    }
    if let Some((day, clock)) = lower.split_once(" at ") {
        let weekday = [
            "monday",
            "tuesday",
            "wednesday",
            "thursday",
            "friday",
            "saturday",
            "sunday",
        ]
        .iter()
        .position(|v| *v == day)?;
        let back = (i64::from(current.weekday().num_days_from_monday())
            - i64::try_from(weekday).ok()?
            + 7)
            % 7;
        return from_unix(
            (current.date_naive() - chrono::Duration::days(back))
                .and_time(parse_clock(clock)?)
                .and_utc()
                .timestamp(),
        );
    }
    let date_formats = if region == Some("UK") {
        ["%d/%m/%Y", "%d-%m-%Y", "%d.%m.%Y"]
    } else {
        ["%m/%d/%Y", "%d-%m-%Y", "%d.%m.%Y"]
    };
    for format in date_formats
        .into_iter()
        .chain(["%b %d %Y", "%d %b %Y", "%B %d, %Y", "%Y-%m-%d"])
    {
        if let Ok(date) = NaiveDate::parse_from_str(input, format) {
            return from_unix(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp());
        }
        for clock in ["%H:%M:%S", "%H:%M", "%I:%M %p"] {
            if let Ok(date) = NaiveDateTime::parse_from_str(input, &format!("{format} {clock}")) {
                return from_unix(date.and_utc().timestamp());
            }
        }
    }
    for format in [
        "MMM d HH:mm",
        "d MMM HH:mm",
        "MM-dd HH:mm",
        "MMM d, yyyy",
        "yyyy-MM-dd HH:mm:ss zzz",
    ] {
        if let Some(date) = layout(input, format, now) {
            return Some(date);
        }
    }
    None
}
fn parse_clock(value: &str) -> Option<NaiveTime> {
    ["%H:%M:%S", "%H:%M", "%I:%M %p", "%I %p"]
        .iter()
        .find_map(|f| NaiveTime::parse_from_str(&value.to_uppercase(), f).ok())
}
