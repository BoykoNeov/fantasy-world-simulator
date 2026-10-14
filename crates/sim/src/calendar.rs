//! The calendar, stored as data. Day 0 is the first day of `first_year`, a first weekday. Weeks run
//! on continuously across month and year boundaries.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CalendarDef {
    pub name: String,
    pub first_year: i64,
    pub weekdays: Vec<String>,
    pub months: Vec<MonthDef>,
    pub seasons: Vec<SeasonDef>,
    pub moons: Vec<MoonDef>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MonthDef {
    pub name: String,
    pub days: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SeasonDef {
    pub name: String,
    pub first_month: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MoonDef {
    pub name: String,
    pub period_days: u32,
    pub offset_days: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Date {
    pub year: i64,
    /// 0-based month index.
    pub month: usize,
    /// 1-based day within the month.
    pub day_of_month: u32,
    /// 0-based day within the year.
    pub day_of_year: u32,
    /// 0-based weekday index.
    pub weekday: usize,
}

#[derive(Clone, Debug)]
pub struct Calendar {
    pub def: CalendarDef,
    /// First day-of-year of each month.
    month_start: Vec<u32>,
    days_per_year: u32,
    /// Season index for each month.
    month_season: Vec<usize>,
}

pub const STANDARD_RON: &str = include_str!("../data/calendar.ron");

impl Calendar {
    /// The calendar shipped in `data/calendar.ron`.
    pub fn standard() -> Self {
        Self::from_ron(STANDARD_RON).expect("bundled calendar is valid")
    }

    pub fn from_ron(text: &str) -> Result<Self, String> {
        let def: CalendarDef = ron::from_str(text).map_err(|e| format!("calendar: {e}"))?;
        Self::new(def)
    }

    pub fn new(def: CalendarDef) -> Result<Self, String> {
        if def.weekdays.is_empty() {
            return Err("calendar: needs at least one weekday".into());
        }
        if def.months.is_empty() || def.months.iter().any(|m| m.days == 0) {
            return Err("calendar: needs months, each with at least one day".into());
        }
        if def.seasons.is_empty() {
            return Err("calendar: needs at least one season".into());
        }
        if def.seasons.windows(2).any(|w| w[0].first_month >= w[1].first_month)
            || def.seasons.iter().any(|s| s.first_month >= def.months.len())
        {
            return Err("calendar: seasons must start in increasing, existing months".into());
        }
        if def.moons.iter().any(|m| m.period_days == 0) {
            return Err("calendar: moon periods must be at least one day".into());
        }
        let mut month_start = Vec::with_capacity(def.months.len());
        let mut acc = 0;
        for m in &def.months {
            month_start.push(acc);
            acc += m.days;
        }
        let month_season = (0..def.months.len())
            .map(|m| def.seasons.iter().rposition(|s| s.first_month <= m).unwrap_or(def.seasons.len() - 1))
            .collect();
        Ok(Calendar { def, month_start, days_per_year: acc, month_season })
    }

    pub fn days_per_year(&self) -> u32 {
        self.days_per_year
    }

    pub fn days_per_week(&self) -> u32 {
        self.def.weekdays.len() as u32
    }

    pub fn months_per_year(&self) -> usize {
        self.def.months.len()
    }

    pub fn date(&self, day: u64) -> Date {
        let dpy = self.days_per_year as u64;
        let year_index = day / dpy;
        let day_of_year = (day % dpy) as u32;
        let month = self.month_start.partition_point(|&s| s <= day_of_year) - 1;
        Date {
            year: self.def.first_year + year_index as i64,
            month,
            day_of_month: day_of_year - self.month_start[month] + 1,
            day_of_year,
            weekday: (day % self.days_per_week() as u64) as usize,
        }
    }

    /// Day number of the first day of a year (counting from `first_year`).
    pub fn year_start(&self, year: i64) -> u64 {
        (year - self.def.first_year) as u64 * self.days_per_year as u64
    }

    pub fn is_week_start(&self, day: u64) -> bool {
        day.is_multiple_of(self.days_per_week() as u64)
    }

    pub fn is_month_start(&self, day: u64) -> bool {
        self.date(day).day_of_month == 1
    }

    pub fn is_year_start(&self, day: u64) -> bool {
        day.is_multiple_of(self.days_per_year as u64)
    }

    /// Fraction of the year elapsed at the middle of a month, in [0, 1).
    pub fn month_mid_year_fraction(&self, month: usize) -> f64 {
        (self.month_start[month] as f64 + self.def.months[month].days as f64 * 0.5) / self.days_per_year as f64
    }

    /// Northern-hemisphere season of a month.
    pub fn season(&self, month: usize) -> usize {
        self.month_season[month]
    }

    /// Moon phase in [0, 1): 0 is new.
    pub fn moon_phase(&self, moon: usize, day: u64) -> f64 {
        let m = &self.def.moons[moon];
        ((day + m.offset_days as u64) % m.period_days as u64) as f64 / m.period_days as f64
    }

    /// e.g. "Hearthday 3 Thaw, year 12".
    pub fn format(&self, day: u64) -> String {
        let d = self.date(day);
        format!("{} {} {}, year {}", self.def.weekdays[d.weekday], d.day_of_month, self.def.months[d.month].name, d.year)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_calendar_loads() {
        let c = Calendar::standard();
        assert_eq!(c.days_per_year(), 12 * 30 + 4);
        assert_eq!(c.days_per_week(), 6);
        assert_eq!(c.def.moons.len(), 2);
    }

    #[test]
    fn dates_walk_forward_without_gaps() {
        let c = Calendar::standard();
        let mut prev = c.date(0);
        assert_eq!((prev.year, prev.month, prev.day_of_month, prev.weekday), (1, 0, 1, 0));
        for day in 1..3 * c.days_per_year() as u64 {
            let d = c.date(day);
            assert_eq!(d.weekday, (prev.weekday + 1) % 6);
            if d.day_of_month == 1 {
                assert!(c.is_month_start(day));
                let expected_month = (prev.month + 1) % c.months_per_year();
                assert_eq!(d.month, expected_month, "day {day}");
                assert_eq!(prev.day_of_month, c.def.months[prev.month].days);
                assert_eq!(d.year, prev.year + (d.month == 0) as i64);
            } else {
                assert_eq!((d.year, d.month, d.day_of_month), (prev.year, prev.month, prev.day_of_month + 1));
            }
            assert_eq!(c.is_year_start(day), d.day_of_year == 0);
            prev = d;
        }
        assert_eq!(c.year_start(3), 2 * 364);
        assert_eq!(c.date(c.year_start(3)).year, 3);
    }

    #[test]
    fn seasons_wrap_around_the_year() {
        let c = Calendar::standard();
        let names: Vec<&str> = (0..c.months_per_year()).map(|m| c.def.seasons[c.season(m)].name.as_str()).collect();
        assert_eq!(names[0], "Winter");
        assert_eq!(names[1], "Winter");
        assert_eq!(names[2], "Spring");
        assert_eq!(names[7], "Summer");
        assert_eq!(names[12], "Winter");
    }

    #[test]
    fn bad_calendars_are_rejected() {
        let mut def = Calendar::standard().def;
        def.months[3].days = 0;
        assert!(Calendar::new(def).is_err());
        let mut def = Calendar::standard().def;
        def.seasons.swap(0, 1);
        assert!(Calendar::new(def).is_err());
        assert!(Calendar::from_ron("(name: \"x\")").is_err());
    }

    #[test]
    fn moon_phases_cycle() {
        let c = Calendar::standard();
        assert_eq!(c.moon_phase(0, 0), 0.0);
        assert_eq!(c.moon_phase(0, 29), 0.0);
        assert!(c.moon_phase(1, 0) > 0.0);
    }
}
