use chrono::{DateTime, Datelike, Duration, FixedOffset, Local, TimeZone, Timelike};
#[cfg(feature = "spin_sleep")]
use spin_sleep::SpinSleeper;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::thread::{self, JoinHandle, ScopedJoinHandle};
#[cfg(all(feature = "spin_sleep", feature = "serde"))]
use {
    serde::{de::VariantAccess, ser::SerializeStructVariant},
    spin_sleep::SpinStrategy,
};
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
    Finished(u64),
}

impl RepetitionCount {
    /// If the repetition's count is finished, then the counter is decremented.
    // The returned bool is the result of a test that checks if the count has reached 0
    fn update_and_check(&mut self) -> bool {
        match self {
            Self::Infinite => false,
            Self::Finished(count) => {
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
    // Used when you need accuracy to the second. In this case, the scheduler uses std::thread::sleep() which has no cost to your program or computer.
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
pub struct TaskScheduled<T> {
    pub task: T,
    pub date: DateTime<FixedOffset>,
    pub repetition: RepetitionType,
    pub sleep_type: SleepType,
}
impl<T> PartialOrd for TaskScheduled<T>
where
    T: Eq,
{
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl<T> Ord for TaskScheduled<T>
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
impl<T> TaskScheduled<T> {
    fn new(
        date: DateTime<FixedOffset>,
        task: T,
        repetition: RepetitionType,
        sleep_type: SleepType,
    ) -> Self {
        Self {
            date,
            task,
            repetition,
            sleep_type,
        }
    }
}
// This struct handles the reading of the Scheduler, meaning that it handles the process of updating the tasks when triggered (ie their dates).
pub struct SchedulerReadingHandler<'pr, T> {
    current_tasks: &'pr mut Vec<TaskScheduled<T>>,
    removed_tasks: Vec<TaskScheduled<T>>,
}
impl<'pr, T: Eq> SchedulerReadingHandler<'pr, T> {
    fn new(current_tasks: &'pr mut Vec<TaskScheduled<T>>) -> Self {
        Self {
            current_tasks,
            removed_tasks: Vec::new(),
        }
    }
    fn get_current_task(&self) -> Option<&TaskScheduled<T>> {
        // The index is always due to the way
        self.current_tasks.get(0)
    }
    fn remove_task(&mut self, index: usize) {
        self.removed_tasks
            .push(self.current_tasks.remove(index));
    }

    fn update_outdated_tasks(&mut self) {
        // Registering outdated tasks
        let now: DateTime<FixedOffset> = Local::now().into();
        let last = self
            .current_tasks
            .iter()
            .position(|task| if now > task.date { false } else { true })
            .unwrap_or(self.current_tasks.len());
        for i in 0..last {
            let task = &mut self.current_tasks[i];
            match &mut task.repetition {
                RepetitionType::Once => {
                    self.remove_task(i);
                }
                RepetitionType::Weekly(_) => {
                    // Important to keep: weekday, time
                    let diff = now - task.date;
                    task.date =
                        task.date + Duration::days(diff.num_days() - diff.num_days() % 7 + 7);
                    // This permits to get the next requestad weekday coming (Mon, Tue, etc...)
                }
                RepetitionType::Monthly(_) => {
                    // Important to keep: month's day, time
                    if (task.date.month0() + 1) % 12 != 2 {
                        // Not February
                        let diff = now - task.date;
                        task.date =
                            task.date + Duration::days(diff.num_days() - diff.num_days() % 7 + 7);
                    } else {
                    }
                }
                RepetitionType::Yearly(_) => {
                    // Important to keep: month, month's day, time
                    let task_day = task.date.day();
                    let task_month = task.date.month();
                    if task_month != 2 && task_day != 29 {
                        // Not February
                        task.date = FixedOffset::east(2 * 3600)
                            .ymd(now.year() + 4, task_month, task_day)
                            .and_hms(
                                task.date.hour(),
                                task.date.minute(),
                                task.date.second(),
                            ); // Leap Year
                    } else {
                        // February
                        task.date = FixedOffset::east(2 * 3600)
                            .ymd(now.year() + 1, task_month, task_day)
                            .and_hms(
                                task.date.hour(),
                                task.date.minute(),
                                task.date.second(),
                            );
                    }
                }
                RepetitionType::Custom { gap, count: _ } => {
                    // Check new count
                    let diff = now - task.date;
                    let deref_gap = *gap;
                    task.date = now
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
    fn update_outdated_tasks_and_repetition_count(&mut self) {
        // Registering outdated tasks
        let now: DateTime<FixedOffset> = Local::now().into();
        let last = self
            .current_tasks
            .iter()
            .position(|task| if now > task.date { false } else { true })
            .unwrap_or(self.current_tasks.len());
        for i in 0..last {
            let task = &mut self.current_tasks[i];
            match &mut task.repetition {
                RepetitionType::Once => {
                    self.remove_task(i);
                }
                RepetitionType::Weekly(count) => {
                    // Check new count
                    if count.update_and_check() {
                        self.remove_task(i);
                        break;
                    }
                    // Important to keep: weekday, time
                    let diff = now - task.date;
                    task.date =
                        task.date + Duration::days(diff.num_days() - diff.num_days() % 7 + 7);
                    // This permits to get the next requestad weekday coming (Mon, Tue, etc...)
                }
                RepetitionType::Monthly(count) => {
                    // Check new count
                    if count.update_and_check() {
                        self.remove_task(i);
                        break;
                    }
                    // Important to keep: month's day, time
                    if (task.date.month0() + 1) % 12 != 2 {
                        // Not February
                        let diff = now - task.date;
                        task.date =
                            task.date + Duration::days(diff.num_days() - diff.num_days() % 7 + 7);
                    } else { // February
                    }
                }
                RepetitionType::Yearly(count) => {
                    // Check new count
                    if count.update_and_check() {
                        self.remove_task(i);
                        break;
                    }
                    // Important to keep: month, month's day, time
                    let task_day = task.date.day();
                    let task_month = task.date.month();
                    if task_month != 2 && task_day != 29 {
                        // Not February
                        task.date = FixedOffset::east(2 * 3600)
                            .ymd(now.year() + 4, task_month, task_day)
                            .and_hms(
                                task.date.hour(),
                                task.date.minute(),
                                task.date.second(),
                            ); // Leap Year
                    } else {
                        // February
                        task.date = FixedOffset::east(2 * 3600)
                            .ymd(now.year() + 1, task_month, task_day)
                            .and_hms(
                                task.date.hour(),
                                task.date.minute(),
                                task.date.second(),
                            );
                    }
                }
                RepetitionType::Custom { gap, count } => {
                    // Check new count
                    if count.update_and_check() {
                        self.remove_task(i);
                        break;
                    }
                    let diff = now - task.date;
                    let deref_gap = *gap;
                    task.date = now
                        + (deref_gap
                            - Duration::milliseconds(
                                // Milliseconds precision, we don't know the need of the user
                                diff.num_milliseconds() % deref_gap.num_milliseconds(),
                            ));
                }
            }
        }
        self.current_tasks.sort();
    }
}

// This is the main
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct BlockingScheduler<T> {
    pub scheduled_tasks: HashMap<String, Vec<TaskScheduled<T>>>,
    pub removed_tasks: HashMap<String, Vec<TaskScheduled<T>>>,
}

impl<T> BlockingScheduler <T>
where
    T: Eq + Default,
{
    pub fn new(
        scheduled_tasks: HashMap<String, Vec<TaskScheduled<T>>>,
        mut removed_tasks: HashMap<String, Vec<TaskScheduled<T>>>,
    ) -> Self {
        Self::format_removed_tasks(&scheduled_tasks, &mut removed_tasks);
        Self {
            scheduled_tasks,
            removed_tasks,
        }
    }

    // This static method permits to be sure that removed_tasks contains all the modes that are presents in scheduled_tasks
    fn format_removed_tasks(
        scheduled_tasks: &HashMap<String, Vec<TaskScheduled<T>>>,
        removed_tasks: &mut HashMap<String, Vec<TaskScheduled<T>>>,
    ) {
        for key in scheduled_tasks.keys() {
            removed_tasks.entry(key.to_owned()).or_insert(Vec::new());
        }
    }

    pub fn start(&mut self, mode: &str, f: fn(&T)) -> Result<(), String> {
        let mut reading_handler = SchedulerReadingHandler::new(
            self.scheduled_tasks
                .get_mut(mode)
                .ok_or(format!("Couldn't find the requested mode : {}", mode))?,
        );
        reading_handler.update_outdated_tasks();
        let mut completed = false;
        while !completed {
            match reading_handler.get_current_task() {
                Some(task) => {
                    let now: DateTime<FixedOffset> = Local::now().into();
                    let diff = (task.date - now).to_std().or(Err(format!(
                        "OutOfRangeError occured on this date {}",
                        &task.date
                    )))?;
                    match task.sleep_type {
                        SleepType::Native => {
                            std::thread::sleep(diff);
                        }
                        #[cfg(feature = "spin_sleep")]
                        SleepType::SpinSleep(spin_sleeper) => {
                            spin_sleeper.sleep(diff);
                        }
                    }
                    f(&task.task);
                    reading_handler.update_outdated_tasks_and_repetition_count();
                }
                None => {
                    completed = true;
                }
            }
        }
        unsafe {
            // This is safe since we applied Self::format_removed_tasks when this struct was constructed
            self.removed_tasks
                .get_mut(mode)
                .unwrap_unchecked()
                .append(&mut reading_handler.removed_tasks);
        }
        Ok(())
    }
}

pub struct ParallelScheduler <'pp, T> {
    scheduler: BlockingScheduler<T>,
    pub thread_handlers: Vec<JoinHandle<Result<(), String>>>,
    pub scope_thread_handlers: Vec<ScopedJoinHandle<'pp, Result<(), String>>>,
}

impl<'pp, T> ParallelScheduler<'pp, T>
where
    T: Eq + Default + Send + Sync + Clone,
{
    pub fn new(
        scheduled_tasks: HashMap<String, Vec<TaskScheduled<T>>>,
        removed_tasks: HashMap<String, Vec<TaskScheduled<T>>>,
    ) -> Self {
        Self {
            scheduler: BlockingScheduler::new(scheduled_tasks, removed_tasks),
            thread_handlers: vec![],
            scope_thread_handlers: vec![],
        }
    }
    pub fn start(&mut self, mode: String, f: fn(&T)) -> std::io::Result<()>
    where
        T: 'static,
    {
        let mut scheduler = self.scheduler.clone();
        self.thread_handlers.push(
            thread::Builder::new()
                .name("ThreadScheduler".to_string())
                .spawn(move || scheduler.start(&mode, f))?,
        );
        Ok(())
    }
    pub fn start_scoped_thread(&mut self, mode: String, f: fn(&T)) -> std::io::Result<()>
    where
        T: 'pp,
    {
        let mut scheduler = self.scheduler.clone();
        thread::scope(|scope| {
            scope.spawn(move || scheduler.start(mode.as_str(), f));
        });
        Ok(())
    }
}
