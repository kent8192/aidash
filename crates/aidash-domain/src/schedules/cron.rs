//! Five-field cron expressions with the usual meaning: minute, hour, day of
//! month, month and day of week, where day 0 and 7 are Sunday and a day
//! matches either day field when both are restricted.
use crate::{Error, Result};
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, TimeZone, Timelike, Utc};
use chrono_tz::Tz;

/// The longest expression stored. Real expressions are a few dozen characters.
pub const MAX_LENGTH: usize = 120;
/// How far ahead the next fire time is looked for. An expression that never
/// fires within it, such as 30 February, is refused.
const SEARCH_DAYS: i64 = 366 * 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cron {
	minutes: u64,
	hours: u64,
	days_of_month: u64,
	months: u64,
	days_of_week: u64,
	/// Both day fields are restricted, so either one matching is enough.
	either_day: bool,
}

const MONTHS: [&str; 12] = [
	"jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
const DAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

fn invalid(expression: &str, why: &str) -> Error {
	Error::Invalid(format!("cron expression {expression:?} is invalid: {why}"))
}

/// One field's allowed values as a bit set, and whether it was restricted
/// (did not start with `*`).
fn field(
	expression: &str,
	text: &str,
	min: u32,
	max: u32,
	names: &[&str],
	name_base: u32,
) -> Result<(u64, bool)> {
	let value = |token: &str| -> Result<u32> {
		if let Some(index) = names
			.iter()
			.position(|name| name.eq_ignore_ascii_case(token))
		{
			return Ok(index as u32 + name_base);
		}
		let number: u32 = token
			.parse()
			.map_err(|_| invalid(expression, &format!("{token:?} is not a number")))?;
		if !(min..=max).contains(&number) {
			return Err(invalid(
				expression,
				&format!("{number} is outside {min} to {max}"),
			));
		}
		Ok(number)
	};
	let mut set = 0_u64;
	for item in text.split(',') {
		let (range, step) = match item.split_once('/') {
			Some((range, step)) => {
				let step: u32 = step
					.parse()
					.ok()
					.filter(|step| *step > 0)
					.ok_or_else(|| invalid(expression, &format!("bad step in {item:?}")))?;
				(range, step)
			}
			None => (item, 1),
		};
		let (start, end) = if range == "*" {
			(min, max)
		} else if let Some((start, end)) = range.split_once('-') {
			(value(start)?, value(end)?)
		} else {
			let start = value(range)?;
			// `5/15` means from 5 to the end in steps of 15.
			(start, if item.contains('/') { max } else { start })
		};
		if start > end {
			return Err(invalid(expression, &format!("{item:?} runs backwards")));
		}
		for number in (start..=end).step_by(step as usize) {
			set |= 1 << number;
		}
	}
	Ok((set, !text.starts_with('*')))
}

impl Cron {
	pub fn parse(expression: &str) -> Result<Self> {
		if expression.len() > MAX_LENGTH {
			return Err(invalid(expression, "it is too long"));
		}
		let fields: Vec<&str> = expression.split_whitespace().collect();
		let [minutes, hours, days_of_month, months, days_of_week] = fields.as_slice() else {
			return Err(invalid(
				expression,
				"it needs five fields: minute, hour, day of month, month and day of week",
			));
		};
		let (minutes, _) = field(expression, minutes, 0, 59, &[], 0)?;
		let (hours, _) = field(expression, hours, 0, 23, &[], 0)?;
		let (days_of_month, dom_restricted) = field(expression, days_of_month, 1, 31, &[], 0)?;
		let (months, _) = field(expression, months, 1, 12, &MONTHS, 1)?;
		let (mut days_of_week, dow_restricted) = field(expression, days_of_week, 0, 7, &DAYS, 0)?;
		// 7 is another name for Sunday.
		if days_of_week & (1 << 7) != 0 {
			days_of_week = (days_of_week & !(1 << 7)) | 1;
		}
		Ok(Self {
			minutes,
			hours,
			days_of_month,
			months,
			days_of_week,
			either_day: dom_restricted && dow_restricted,
		})
	}

	/// The shortest gap between two fire times in local time, in minutes:
	/// between two times of one day, and from the last time of a day to the
	/// first of the next, as if any two days could both match. A
	/// daylight-saving change can shorten a gap once; the scheduler also
	/// refuses a tick sooner than the minimum interval after the previous one.
	pub fn shortest_local_gap_minutes(&self) -> i64 {
		let times: Vec<i64> = (0..24_i64)
			.filter(|hour| self.hours & (1 << hour) != 0)
			.flat_map(|hour| {
				(0..60_i64)
					.filter(|minute| self.minutes & (1 << minute) != 0)
					.map(move |minute| hour * 60 + minute)
			})
			.collect();
		let (Some(first), Some(last)) = (times.first(), times.last()) else {
			return i64::MAX;
		};
		times
			.windows(2)
			.map(|pair| pair[1] - pair[0])
			.fold(first + 24 * 60 - last, i64::min)
	}

	fn day_matches(&self, date: NaiveDate) -> bool {
		if self.months & (1 << date.month()) == 0 {
			return false;
		}
		let month_day = self.days_of_month & (1 << date.day()) != 0;
		let week_day = self.days_of_week & (1 << date.weekday().num_days_from_sunday()) != 0;
		if self.either_day {
			month_day || week_day
		} else {
			month_day && week_day
		}
	}

	/// The first fire time strictly after `after`, read in `zone`. A local time
	/// that a daylight-saving change skips does not fire; one it repeats fires
	/// at its first occurrence.
	pub fn next_after(&self, zone: Tz, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
		let local = after.with_timezone(&zone).naive_local();
		let start_day = local.date();
		for offset in 0..SEARCH_DAYS {
			let date = start_day + Duration::days(offset);
			if !self.day_matches(date) {
				continue;
			}
			for hour in (0..24).filter(|hour| self.hours & (1 << hour) != 0) {
				for minute in (0..60).filter(|minute| self.minutes & (1 << minute) != 0) {
					let time = NaiveTime::from_hms_opt(hour, minute, 0)?;
					let candidate = date.and_time(time);
					// Earlier local times of the first day cannot follow `after`,
					// except across a repeated hour, which the comparison below
					// settles in UTC.
					if offset == 0 && candidate + Duration::hours(3) < local {
						continue;
					}
					let Some(at) = zone.from_local_datetime(&candidate).earliest() else {
						continue;
					};
					let at = at.with_timezone(&Utc);
					if at > after {
						return Some(at);
					}
				}
			}
		}
		None
	}
}

/// A time zone name from the IANA database, such as `Asia/Tokyo` or `UTC`.
pub fn time_zone(name: &str) -> Result<Tz> {
	name.parse::<Tz>()
		.map_err(|_| Error::Invalid(format!("{name:?} is not a known time zone")))
}

/// Whole minutes only: the expression has no seconds field.
pub fn minute_floor(at: DateTime<Utc>) -> DateTime<Utc> {
	at.with_second(0)
		.and_then(|at| at.with_nanosecond(0))
		.unwrap_or(at)
}
