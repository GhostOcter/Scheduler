use chrono::{DateTime, Datelike, Duration, FixedOffset, Local, TimeZone, Timelike};
#[cfg(feature = "spin_sleep")]
use spin_sleep::SpinSleeper;
#[cfg(all(feature = "spin_sleep", feature = "serde"))]
use {spin_sleep::SpinStrategy, serde::{ser::SerializeStructVariant, de::VariantAccess}};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::thread::{self, JoinHandle, ScopedJoinHandle};
#[cfg(feature = "serde")]
use {
    serde::{
        de::{EnumAccess, Visitor},
        Deserialize, Serialize,
    },
    serde_with::{As, DurationSeconds},
};

/// Represents the number of times the repetitions will occurs
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(PartialEq, Eq, Clone, Debug, Default)]
pub enum RepetitionCount {
    #[default]
    Infinite,
    Custom(u64),
}

impl RepetitionCount {
    fn update_and_check(&mut self) -> bool {
        match self {
            Self::Infinite => false,
            Self::Custom(count) => {
                *count -= 1;
                *count <= 0
            }
        }
    }
}
/// Represents how the date will be repeated
/// - Once
/// - Weekly
/// - Monthly
/// - Yearly
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
    Custom {
        #[cfg_attr(feature = "serde", serde(with = "As::<DurationSeconds<i64>>"))]
        gap: Duration,
        count: RepetitionCount,
    },
}
// You need to know that the
#[derive(PartialEq, Eq, Clone, Debug, Default)]
pub enum SleepType {
    #[default]
    // Accurate to the second => Use std::thread::sleep =>
    Native,
    // Accurate to the millisecond => Use spin sleep which require more ressoruces to work
    #[cfg(feature = "spin_sleep")]
    SpinSleep(SpinSleeper),
}
#[cfg(feature = "serde")]
impl Serialize for SleepType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match &self {
            Self::Native => serializer.serialize_unit_variant("SleepType", 0, "Native"),
            #[cfg(feature = "spin_sleep")]
            Self::SpinSleep(spin_sleeper) => {
                let mut sv = serializer.serialize_struct_variant("SleepType", 1, "SpinSleep", 2)?;
                sv.serialize_field(
                    "native_accuracy_ns",
                    &spin_sleeper.clone().native_accuracy_ns(),
                )?;
                sv.serialize_field(
                    "spin_strategy",
                    if spin_sleeper.spin_strategy() == SpinStrategy::YieldThread {
                        &0
                    } else {
                        &1
                    },
                )?;
                sv.end()
            }
        }
    }
}

#[cfg(feature = "serde")]
impl<'de> Deserialize<'de> for SleepType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct SleepVisitor;
        impl<'de> Visitor<'de> for SleepVisitor {
            type Value = SleepType;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("Expecting serialized SleepType enum")
            }
            #[cfg(not(feature = "spin_sleep"))]
            fn visit_enum<A>(self, _: A) -> Result<Self::Value, A::Error>
            where
                A: EnumAccess<'de>,
            {
                Ok(SleepType::Native)
            }
            #[cfg(feature = "spin_sleep")]
            fn visit_enum<A>(self, data: A) -> Result<Self::Value, A::Error>
            where
                A: EnumAccess<'de>,
            {
                let variant = data.variant::<String>()?;
                if variant.0 == "Native" {
                    Ok(SleepType::Native)
                } else {
                    Ok(variant
                        .1
                        .struct_variant(&["native_accuracy_ns", "spin_strategy"], Self)?)
                }
            }

            #[cfg(feature = "spin_sleep")]
            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                Ok(SleepType::SpinSleep(
                    SpinSleeper::new(
                        map.next_entry::<String, u32>()?
                            .expect("Native accuracy field")
                            .1,
                    )
                    .with_spin_strategy(
                        if map
                            .next_entry::<String, u8>()?
                            .expect("Spin strategy field")
                            .1
                            == 0
                        {
                            SpinStrategy::YieldThread
                        } else {
                            SpinStrategy::SpinLoopHint
                        },
                    ),
                ))
            }
        }
        deserializer.deserialize_enum("SleepType", &["Native", "SpinSleep"], SleepVisitor)
    }
}
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct ActionPlanned<T> {
    pub action: T,
    pub date: DateTime<FixedOffset>,
    pub repetition: RepetitionType,
    pub sleep_type: SleepType,
}
impl<T> PartialOrd for ActionPlanned<T>
where
    T: Eq,
{
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl<T> Ord for ActionPlanned<T>
where
    T: Eq,
{
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        if self.date < other.date {
            Ordering::Less
        } else if self.date > other.date {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    }
}
impl<T> ActionPlanned<T> {
    fn new(
        date: DateTime<FixedOffset>,
        action: T,
        repetition: RepetitionType,
        sleep_type: SleepType,
    ) -> Self {
        Self {
            date,
            action,
            repetition,
            sleep_type,
        }
    }
}
// This struct handles the reading of the planner, meaning that it handles the process of updating the actions when triggered (ie their dates).
pub struct PlannerReadingHandler<'pr, T> {
    current_actions: &'pr mut Vec<ActionPlanned<T>>,
    removed_actions: Vec<ActionPlanned<T>>,
}
impl<'pr, T: Eq> PlannerReadingHandler<'pr, T> {
    fn new(current_actions: &'pr mut Vec<ActionPlanned<T>>) -> Self {
        Self {
            current_actions,
            removed_actions: Vec::new(),
        }
    }
    fn get_current_action(&self) -> Option<&ActionPlanned<T>> {
        // The index is always due to the way
        self.current_actions.get(0)
    }
    fn remove_action(&mut self, index: usize) {
        self.removed_actions
            .push(self.current_actions.remove(index));
    }

    fn update_outdated_actions(&mut self) {
        // Registering outdated actions
        let now: DateTime<FixedOffset> = Local::now().into();
        let last = self
            .current_actions
            .iter()
            .position(|action| if now > action.date { false } else { true })
            .unwrap_or(self.current_actions.len());
        for i in 0..last {
            let action = &mut self.current_actions[i];
            match &mut action.repetition {
                RepetitionType::Once => {
                    self.remove_action(i);
                }
                RepetitionType::Weekly(_) => {
                    // Important to keep: weekday, time
                    let diff = now - action.date;
                    action.date =
                        action.date + Duration::days(diff.num_days() - diff.num_days() % 7 + 7);
                    // This permits to get the next requestad weekday coming (Mon, Tue, etc...)
                }
                RepetitionType::Monthly(_) => {
                    // Important to keep: month's day, time
                    if (action.date.month0() + 1) % 12 != 2 {
                        // Not February
                        let diff = now - action.date;
                        action.date =
                            action.date + Duration::days(diff.num_days() - diff.num_days() % 7 + 7);
                    } else {
                    }
                }
                RepetitionType::Yearly(_) => {
                    // Important to keep: month, month's day, time
                    let action_day = action.date.day();
                    let action_month = action.date.month();
                    if action_month != 2 && action_day != 29 {
                        // Not February
                        action.date = FixedOffset::east(2 * 3600)
                            .ymd(now.year() + 4, action_month, action_day)
                            .and_hms(
                                action.date.hour(),
                                action.date.minute(),
                                action.date.second(),
                            ); // Leap Year
                    } else {
                        // February
                        action.date = FixedOffset::east(2 * 3600)
                            .ymd(now.year() + 1, action_month, action_day)
                            .and_hms(
                                action.date.hour(),
                                action.date.minute(),
                                action.date.second(),
                            );
                    }
                }
                RepetitionType::Custom { gap, count: _ } => {
                    // Check new count
                    let diff = now - action.date;
                    let deref_gap = *gap;
                    action.date = now
                        + (deref_gap
                            - Duration::milliseconds(
                                // Milliseconds precision, we don't know the need of the user
                                diff.num_milliseconds() % deref_gap.num_milliseconds(),
                            ));
                }
            }
        }
    }
    //#TODO: FINISH YEARLY AND MONTHLY
    fn update_outdated_actions_and_repetition_count(&mut self) {
        // Registering outdated actions
        let now: DateTime<FixedOffset> = Local::now().into();
        let last = self
            .current_actions
            .iter()
            .position(|action| if now > action.date { false } else { true })
            .unwrap_or(self.current_actions.len());
        for i in 0..last {
            let action = &mut self.current_actions[i];
            match &mut action.repetition {
                RepetitionType::Once => {
                    self.remove_action(i);
                }
                RepetitionType::Weekly(count) => {
                    // Check new count
                    if count.update_and_check() {
                        self.remove_action(i);
                        break;
                    }
                    // Important to keep: weekday, time
                    let diff = now - action.date;
                    action.date =
                        action.date + Duration::days(diff.num_days() - diff.num_days() % 7 + 7);
                    // This permits to get the next requestad weekday coming (Mon, Tue, etc...)
                }
                RepetitionType::Monthly(count) => {
                    // Check new count
                    if count.update_and_check() {
                        self.remove_action(i);
                        break;
                    }
                    // Important to keep: month's day, time
                    if (action.date.month0() + 1) % 12 != 2 {
                        // Not February
                        let diff = now - action.date;
                        action.date =
                            action.date + Duration::days(diff.num_days() - diff.num_days() % 7 + 7);
                    } else { // February
                    }
                }
                RepetitionType::Yearly(count) => {
                    // Check new count
                    if count.update_and_check() {
                        self.remove_action(i);
                        break;
                    }
                    // Important to keep: month, month's day, time
                    let action_day = action.date.day();
                    let action_month = action.date.month();
                    if action_month != 2 && action_day != 29 {
                        // Not February
                        action.date = FixedOffset::east(2 * 3600)
                            .ymd(now.year() + 4, action_month, action_day)
                            .and_hms(
                                action.date.hour(),
                                action.date.minute(),
                                action.date.second(),
                            ); // Leap Year
                    } else {
                        // February
                        action.date = FixedOffset::east(2 * 3600)
                            .ymd(now.year() + 1, action_month, action_day)
                            .and_hms(
                                action.date.hour(),
                                action.date.minute(),
                                action.date.second(),
                            );
                    }
                }
                RepetitionType::Custom { gap, count } => {
                    // Check new count
                    if count.update_and_check() {
                        self.remove_action(i);
                        break;
                    }
                    let diff = now - action.date;
                    let deref_gap = *gap;
                    action.date = now
                        + (deref_gap
                            - Duration::milliseconds(
                                // Milliseconds precision, we don't know the need of the user
                                diff.num_milliseconds() % deref_gap.num_milliseconds(),
                            ));
                }
            }
        }
        self.current_actions.sort();
    }
}

// This is the main
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct BlockingPlanner<T> {
    pub planned_actions: HashMap<String, Vec<ActionPlanned<T>>>,
    pub removed_actions: HashMap<String, Vec<ActionPlanned<T>>>,
}

impl<T> BlockingPlanner<T>
where
    T: Eq + Default,
{
    pub fn new(
        planned_actions: HashMap<String, Vec<ActionPlanned<T>>>,
        mut removed_actions: HashMap<String, Vec<ActionPlanned<T>>>,
    ) -> Self {
        Self::format_removed_actions(&planned_actions, &mut removed_actions);
        Self {
            planned_actions: planned_actions,
            removed_actions,
        }
    }

    // This static method permits to be sure that removed_actions contains all the modes that are presents in planned_actions
    fn format_removed_actions(
        planned_actions: &HashMap<String, Vec<ActionPlanned<T>>>,
        removed_actions: &mut HashMap<String, Vec<ActionPlanned<T>>>,
    ) {
        for key in planned_actions.keys() {
            removed_actions.entry(key.to_owned()).or_insert(Vec::new());
        }
    }

    pub fn start(&mut self, mode: &str, f: fn(&T)) -> Result<(), String> {
        let mut reading_handler = PlannerReadingHandler::new(
            self.planned_actions
                .get_mut(mode)
                .ok_or(format!("Couldn't find the requested mode : {}", mode))?,
        );
        reading_handler.update_outdated_actions();
        let mut completed = false;
        while !completed {
            match reading_handler.get_current_action() {
                Some(action) => {
                    let now: DateTime<FixedOffset> = Local::now().into();
                    let diff = (action.date - now).to_std().or(Err(format!(
                        "OutOfRangeError occured on this date {}",
                        &action.date
                    )))?;
                    match action.sleep_type {
                        SleepType::Native => {
                            std::thread::sleep(diff);
                        }
                        #[cfg(feature = "spin_sleep")]
                        SleepType::SpinSleep(spin_sleeper) => {
                            spin_sleeper.sleep(diff);
                        }
                    }
                    f(&action.action);
                    reading_handler.update_outdated_actions_and_repetition_count();
                }
                None => {
                    completed = true;
                }
            }
        }
        unsafe {
            // This is safe since we applied Self::format_removed_actions when this struct was constructed
            self.removed_actions
                .get_mut(mode)
                .unwrap_unchecked()
                .append(&mut reading_handler.removed_actions);
        }
        Ok(())
    }
}

pub struct ParallelPlanner<'pp, T> {
    planner: BlockingPlanner<T>,
    pub thread_handlers: Vec<JoinHandle<Result<(), String>>>,
    pub scope_thread_handlers: Vec<ScopedJoinHandle<'pp, Result<(), String>>>,
}

impl<'pp, T> ParallelPlanner<'pp, T>
where
    T: Eq + Default + Send + Sync + Clone,
{
    pub fn new(
        planned_actions: HashMap<String, Vec<ActionPlanned<T>>>,
        removed_actions: HashMap<String, Vec<ActionPlanned<T>>>,
    ) -> Self {
        Self {
            planner: BlockingPlanner::new(planned_actions, removed_actions),
            thread_handlers: vec![],
            scope_thread_handlers: vec![],
        }
    }
    pub fn start(&mut self, mode: String, f: fn(&T)) -> std::io::Result<()>
    where
        T: 'static,
    {
        let mut planner = self.planner.clone();
        self.thread_handlers.push(
            thread::Builder::new()
                .name("ThreadPlanner".to_string())
                .spawn(move || planner.start(&mode, f))?,
        );
        Ok(())
    }
    pub fn start_scoped_thread(&mut self, mode: String, f: fn(&T)) -> std::io::Result<()>
    where
        T: 'pp,
    {
        let mut planner = self.planner.clone();
        thread::scope(|scope| {
            scope.spawn(move || planner.start(mode.as_str(), f));
        });
        Ok(())
    }
}
