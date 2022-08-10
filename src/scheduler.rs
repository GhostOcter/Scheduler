use chrono::{DateTime, Datelike, Duration, FixedOffset, Local, TimeZone, Timelike};
#[cfg(feature = "spin_sleep")]
use spin_sleep::SpinSleeper;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt::Debug;
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
    fn is_finished_on_update(&mut self) -> bool {
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
    fn update_date(&self, origin: &DateTime<FixedOffset>, current_date: &DateTime<FixedOffset>) -> Option<DateTime<FixedOffset>>;
}
#[derive(Clone, Debug)]
pub struct NoCustomRepetition;

impl CustomRepetition for NoCustomRepetition {
    fn update_date(&self, _: &DateTime<FixedOffset>, _: &DateTime<FixedOffset>) -> Option<DateTime<FixedOffset>> {
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
pub enum RepetitionType
{
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
            if updated_month == 1  {
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
pub struct ScheduledTask<TaskType> {
    pub task: TaskType,
    pub date: DateTime<FixedOffset>,
    pub repetition: RepetitionType,
    pub sleep_type: SleepType,
}
impl<TaskType> PartialOrd for ScheduledTask<TaskType>
where
    TaskType: Eq,
{
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl<TaskType> Ord for ScheduledTask<TaskType>
where
    TaskType: Eq,
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
impl<TaskType> ScheduledTask<TaskType> {
    fn new(
        date: DateTime<FixedOffset>,
        task: TaskType,
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
pub struct SchedulerReadingHandler<'srh, TaskType, RepetitionHandlerType = NoCustomRepetition> {
    current_tasks: &'srh mut Vec<ScheduledTask<TaskType>>,
    removed_tasks: Vec<ScheduledTask<TaskType>>,
    repetition_handler: RepetitionHandlerType
}

impl<'srh, TaskType, RepetitionHandlerType> SchedulerReadingHandler<'srh, TaskType, RepetitionHandlerType> 
where TaskType: Eq,
      RepetitionHandlerType: CustomRepetition
{
    fn new(current_tasks: &'srh mut Vec<ScheduledTask<TaskType>>, repetition_handler: RepetitionHandlerType) -> Self {
        Self {
            current_tasks,
            removed_tasks: Vec::new(),
            repetition_handler
        }
    }
    fn get_current_task(&self) -> Option<&ScheduledTask<TaskType>> {
        // The index is always due to the way
        self.current_tasks.get(0)
    }
    fn remove_task(&mut self, index: usize) {
        self.removed_tasks.push(self.current_tasks.remove(index));
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
                    RepetitionHelpers::update_weekly(&now, &mut task.date);
                }
                RepetitionType::Monthly(_) => {
                    // Important to keep: month's day, time
                    RepetitionHelpers::update_monthly(&now, &mut task.date);
                }
                RepetitionType::Yearly(_) => {
                    RepetitionHelpers::update_yearly(&now, &mut task.date)
                }
                RepetitionType::ConstGap { gap, count: _ } => {
                    RepetitionHelpers::update_const_gap(&now, &mut task.date, *gap);
                }
                RepetitionType::Custom => {
                    if let Some(new_date) = self.repetition_handler.update_date(&now, &task.date){
                        task.date = new_date;
                    } else {
                        self.remove_task(i);
                    }
                }
            }
        }
        }

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
                    if count.is_finished_on_update() {
                        self.remove_task(i);
                        break;
                    }
                    RepetitionHelpers::update_weekly(&now, &mut task.date);

                }
                RepetitionType::Monthly(count) => {
                    // Check new count
                    if count.is_finished_on_update() {
                        self.remove_task(i);
                        break;
                    }
                    RepetitionHelpers::update_monthly(&now, &mut task.date);
                }
                RepetitionType::Yearly(count) => {
                    // Check new count
                    if count.is_finished_on_update() {
                        self.remove_task(i);
                        break;
                    }
                    RepetitionHelpers::update_yearly(&now, &mut task.date)
                }
                RepetitionType::ConstGap { gap, count } => {
                    // Check new count
                    if count.is_finished_on_update() {
                        self.remove_task(i);
                        break;
                    }
                    RepetitionHelpers::update_const_gap(&now, &mut task.date, *gap);
                }
                RepetitionType::Custom => {
                    if let Some(new_date) = self.repetition_handler.update_date(&now, &task.date){
                        task.date = new_date;
                    } else {
                        self.remove_task(i);
                    }
                }
            }
        }
        self.current_tasks.sort();
    }
}

struct SchedulerHelper;
impl SchedulerHelper {
      // This static method permits to be sure that removed_tasks contains all the modes that are presents in scheduled_tasks
    fn format_removed_tasks<TaskType, RepetitionType>(
        scheduled_tasks: &HashMap<String, Vec<ScheduledTask<TaskType>>>,
        removed_tasks: &mut HashMap<String, Vec<ScheduledTask<TaskType>>>,
    ) {
        for key in scheduled_tasks.keys() {
            removed_tasks.entry(key.to_owned()).or_insert(Vec::new());
        }
    }
}
// This is the main
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
pub struct BlockingScheduler<TaskType, CustomRepetitionType = NoCustomRepetition> {
    pub scheduled_tasks: HashMap<String, Vec<ScheduledTask<TaskType>>>,
    pub removed_tasks: HashMap<String, Vec<ScheduledTask<TaskType>>>,

    custom_repetition: CustomRepetitionType
}

impl<TaskType> BlockingScheduler<TaskType, NoCustomRepetition>
where TaskType: Eq + Default 
{
    pub fn new(
        scheduled_tasks: HashMap<String, Vec<ScheduledTask<TaskType>>>,
        mut removed_tasks: HashMap<String, Vec<ScheduledTask<TaskType>>>,
    ) -> Self {
        SchedulerHelper::format_removed_tasks::<TaskType, NoCustomRepetition>(&scheduled_tasks, &mut removed_tasks);
        Self {
            scheduled_tasks,
            removed_tasks,
            custom_repetition: NoCustomRepetition
        }
    }

}

impl<TaskType, CustomRepetitionType> BlockingScheduler<TaskType, CustomRepetitionType>
where
    TaskType: Eq + Default,
    CustomRepetitionType: CustomRepetition + Clone
{
    fn new_with_custom_repetition(
        scheduled_tasks: HashMap<String, Vec<ScheduledTask<TaskType>>>,
        mut removed_tasks: HashMap<String, Vec<ScheduledTask<TaskType>>>,
        custom_repetition: CustomRepetitionType
    ) -> Self {
        SchedulerHelper::format_removed_tasks::<TaskType, CustomRepetitionType>(&scheduled_tasks, &mut removed_tasks);
        Self { scheduled_tasks, removed_tasks, custom_repetition }
    }

    pub fn start(&mut self, mode: &str, f: fn(&TaskType)) -> Result<(), String> {
        let mut reading_handler = SchedulerReadingHandler::new(
            self.scheduled_tasks
                .get_mut(mode)
                .ok_or(format!("Couldn't find the requested mode : {}", mode))?,
                self.custom_repetition.clone()
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

pub struct ParallelScheduler<'ps, TaskType, CustomRepetition = NoCustomRepetition> {
    scheduler: BlockingScheduler<TaskType, CustomRepetition>,
    pub thread_handlers: Vec<JoinHandle<Result<(), String>>>,
    pub scope_thread_handlers: Vec<ScopedJoinHandle<'ps, Result<(), String>>>,
}
impl<'ps, TaskType> ParallelScheduler<'ps, TaskType, NoCustomRepetition>
where TaskType: Eq + Default
{
    pub fn new(
        scheduled_tasks: HashMap<String, Vec<ScheduledTask<TaskType>>>,
        removed_tasks: HashMap<String, Vec<ScheduledTask<TaskType>>>,
    ) -> Self {
        Self {
            scheduler: BlockingScheduler::new(scheduled_tasks, removed_tasks),
            scope_thread_handlers: vec![],
            thread_handlers: vec![]
        }
    }

}

impl<'ps, TaskType, CustomRepetitionType> ParallelScheduler<'ps, TaskType, CustomRepetitionType>
where
    TaskType: Eq + Default + Send + Sync + Clone,
    CustomRepetitionType: CustomRepetition + Clone + Send + Sync
{
   
    pub fn new_with_custom_repetition(
        scheduled_tasks: HashMap<String, Vec<ScheduledTask<TaskType>>>,
        removed_tasks: HashMap<String, Vec<ScheduledTask<TaskType>>>,
        custom_repetition: CustomRepetitionType
    ) -> Self {
        Self {
            scheduler: BlockingScheduler::new_with_custom_repetition(scheduled_tasks, removed_tasks, custom_repetition),
            scope_thread_handlers: vec![],
            thread_handlers: vec![]
        }
    }

    pub fn start(&mut self, mode: String, f: fn(&TaskType)) -> std::io::Result<()>
    where
        TaskType: 'static,
        CustomRepetitionType: 'static
    {
        let mut scheduler = self.scheduler.clone();
        self.thread_handlers.push(
            thread::Builder::new()
                .name("ThreadScheduler".to_string())
                .spawn(move || scheduler.start(&mode, f))?,
        );
        Ok(())
    }
    pub fn start_scoped_thread(&mut self, mode: String, f: fn(&TaskType)) -> std::io::Result<()>
    where
        TaskType: 'ps,
        CustomRepetitionType: 'ps
    {
        let mut scheduler = self.scheduler.clone();
        thread::scope(|scope| {
            scope.spawn(move || scheduler.start(mode.as_str(), f));
        });
        Ok(())
    }
}
