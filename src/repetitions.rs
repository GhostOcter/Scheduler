use chrono::{DateTime, Datelike, Duration, FixedOffset, TimeZone, Timelike};
#[cfg(feature = "serde")]
use {
    serde::{Deserialize, Serialize},
    serde_with::{As, DurationSeconds},
};

/// Represents the number of times the repetitions will occurs
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(PartialEq, Eq, Clone, Debug, Default)]
pub enum RepetitionCount {
    #[default]
    Infinite,
    Finished(u64),
}

impl RepetitionCount {
    /// If the repetition's count is finished, then the counter is decremented.
    // The returned bool is the result of a test that checks if the count has reached 0
    pub(crate) fn is_finished_on_update(&mut self) -> bool {
        match self {
            Self::Infinite => false,
            Self::Finished(count) => {
                *count -= 1;
                *count <= 0
            }
        }
    }
}
pub trait CustomRepetition {
    fn update_date(
        &self,
        origin: &DateTime<FixedOffset>,
        current_date: &DateTime<FixedOffset>,
    ) -> Option<DateTime<FixedOffset>>;
}
#[derive(Clone, Debug)]
pub struct NoCustomRepetition;

impl CustomRepetition for NoCustomRepetition {
    fn update_date(
        &self,
        _: &DateTime<FixedOffset>,
        _: &DateTime<FixedOffset>,
    ) -> Option<DateTime<FixedOffset>> {
        panic!("Dynamic gap encountered, but no RepetitionHandler has been found...")
    }
}
/// Represents how the date will be repeated
/// - Once
/// - Weekly
/// - Monthly
/// - Yearly
/// - StaticGap
/// - Custom : the gap represents the amount of time between two repetitions
/// For Weekly, Monthly, Yearly and Custom, you need to give a RepetitionCount
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(PartialEq, Eq, Clone, Debug, Default)]
pub enum RepetitionType {
    #[default]
    Once,
    Weekly(RepetitionCount),
    Monthly(RepetitionCount),
    Yearly(RepetitionCount),
    ConstGap {
        #[cfg_attr(feature = "serde", serde(with = "As::<DurationSeconds<i64>>"))]
        gap: Duration,
        count: RepetitionCount,
    },
    Custom,
}
pub struct RepetitionHelpers;
impl RepetitionHelpers {
    pub fn update_weekly(origin: &DateTime<FixedOffset>, date: &mut DateTime<FixedOffset>) {
        Self::update_const_gap(origin, date, Duration::days(7));
    }
    pub fn update_monthly(origin: &DateTime<FixedOffset>, date: &mut DateTime<FixedOffset>) {
        let updated_month = {
            if origin.day() > date.day() {
                (origin.month() + 1) % 12
            } else {
                origin.month()
            }
        };
        let updated_year = {
            if updated_month == 1 {
                origin.year() + 1
            } else {
                origin.year()
            }
        };
        *date = FixedOffset::east(2 * 3600)
            .ymd(updated_year, updated_month, date.day())
            .and_hms(date.hour(), date.minute(), date.second());
    }
    pub fn update_yearly(origin: &DateTime<FixedOffset>, date: &mut DateTime<FixedOffset>) {
        // Important to keep: month, month's day, time
        // + take care of leap year
        let day = date.day();
        let month = date.month();
        if month != 2 && day != 29 {
            // Not 29 February
            *date = FixedOffset::east(2 * 3600)
                .ymd(origin.year() + 1, month, day)
                .and_hms(date.hour(), date.minute(), date.second());
        } else {
            // 29 February => Leap year
            *date = FixedOffset::east(2 * 3600)
                .ymd(
                    (origin.year() - date.year()) % 4 + origin.year(),
                    month,
                    day,
                )
                .and_hms(date.hour(), date.minute(), date.second()); // Leap Year
        }
    }
    //TODO: Rethink about the name of this method and its associated variant
    pub fn update_const_gap(
        origin: &DateTime<FixedOffset>,
        date: &mut DateTime<FixedOffset>,
        gap: Duration,
    ) {
        // Check new count
        let diff = *origin - *date;
        *date = *origin
            + (gap
                - Duration::milliseconds(
                    // Milliseconds precision, we don't know the need of the user
                    diff.num_milliseconds() % gap.num_milliseconds(),
                ));
    }
}
