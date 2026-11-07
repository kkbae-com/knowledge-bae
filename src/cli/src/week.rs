//! Which days make up "this week" and "last week" for timesheet summaries.

use anyhow::{bail, Result};
use chrono::{Datelike, Duration, NaiveDate, Weekday};

/// Which week to report on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WeekSel {
    This,
    Last,
    /// The week that contains this date.
    Containing(NaiveDate),
}

/// Parses a full English weekday name, case-insensitively.
pub fn parse_weekday(s: &str) -> Result<Weekday> {
    match s.trim().to_lowercase().as_str() {
        "sunday" => Ok(Weekday::Sun),
        "monday" => Ok(Weekday::Mon),
        "tuesday" => Ok(Weekday::Tue),
        "wednesday" => Ok(Weekday::Wed),
        "thursday" => Ok(Weekday::Thu),
        "friday" => Ok(Weekday::Fri),
        "saturday" => Ok(Weekday::Sat),
        _ => bail!("'{s}' is not a day of the week (use sunday, monday, tuesday, wednesday, thursday, friday or saturday)"),
    }
}

/// Parses `this`, `last` or a `YYYY-MM-DD` date (the week containing it).
pub fn parse_week_sel(s: &str) -> Result<WeekSel> {
    match s.trim().to_lowercase().as_str() {
        "this" => Ok(WeekSel::This),
        "last" => Ok(WeekSel::Last),
        other => NaiveDate::parse_from_str(other, "%Y-%m-%d")
            .map(WeekSel::Containing)
            .map_err(|_| anyhow::anyhow!("'{s}' is not a week: use this, last or a date like YYYY-MM-DD")),
    }
}

/// The first and last day (inclusive, seven days apart minus one) of the selected week.
pub fn week_bounds(today: NaiveDate, start: Weekday, sel: WeekSel) -> (NaiveDate, NaiveDate) {
    let anchor = match sel {
        WeekSel::This => today,
        WeekSel::Last => today - Duration::days(7),
        WeekSel::Containing(date) => date,
    };
    let since_start = (anchor.weekday().num_days_from_sunday() + 7 - start.num_days_from_sunday()) % 7;
    let first = anchor - Duration::days(i64::from(since_start));
    (first, first + Duration::days(6))
}

/// Today's date on this machine.
pub fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn sunday_start_this_and_last_week() {
        let today = d(2026, 11, 7); // a Saturday
        assert_eq!(week_bounds(today, Weekday::Sun, WeekSel::This), (d(2026, 11, 1), d(2026, 11, 7)));
        assert_eq!(week_bounds(today, Weekday::Sun, WeekSel::Last), (d(2026, 10, 25), d(2026, 10, 31)));
    }

    #[test]
    fn today_being_the_start_day_starts_a_new_week() {
        let today = d(2026, 11, 8); // a Sunday
        assert_eq!(week_bounds(today, Weekday::Sun, WeekSel::This), (d(2026, 11, 8), d(2026, 11, 14)));
        assert_eq!(week_bounds(today, Weekday::Sun, WeekSel::Last), (d(2026, 11, 1), d(2026, 11, 7)));
    }

    #[test]
    fn monday_start_changes_the_boundaries() {
        assert_eq!(
            week_bounds(d(2026, 11, 8), Weekday::Mon, WeekSel::This),
            (d(2026, 11, 2), d(2026, 11, 8))
        );
    }

    #[test]
    fn a_date_selects_its_own_week() {
        assert_eq!(
            week_bounds(d(2026, 12, 25), Weekday::Sun, WeekSel::Containing(d(2026, 11, 3))),
            (d(2026, 11, 1), d(2026, 11, 7))
        );
    }

    #[test]
    fn week_selectors_parse() {
        assert_eq!(parse_week_sel("this").unwrap(), WeekSel::This);
        assert_eq!(parse_week_sel("Last").unwrap(), WeekSel::Last);
        assert_eq!(parse_week_sel("2026-11-03").unwrap(), WeekSel::Containing(d(2026, 11, 3)));
        let err = parse_week_sel("tomorrow").unwrap_err().to_string();
        assert!(err.contains("this") && err.contains("YYYY-MM-DD"), "{err}");
    }

    #[test]
    fn weekday_names_parse_and_bad_ones_list_the_choices() {
        assert_eq!(parse_weekday("Monday").unwrap(), Weekday::Mon);
        assert_eq!(parse_weekday(" sunday ").unwrap(), Weekday::Sun);
        let err = parse_weekday("funday").unwrap_err().to_string();
        assert!(err.contains("sunday") && err.contains("funday"), "{err}");
    }
}
