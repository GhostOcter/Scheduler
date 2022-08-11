use super::repetitions::{CustomRepetition, NoCustomRepetition, RepetitionHelpers, RepetitionType};
use super::sleeptype::SleepType;
use chrono::{DateTime, FixedOffset, Local};
#[cfg(feature = "spin_sleep")]
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
    repetition_handler: RepetitionHandlerType,
}

impl<'srh, TaskType, RepetitionHandlerType>
    SchedulerReadingHandler<'srh, TaskType, RepetitionHandlerType>
where
    TaskType: Eq,
    RepetitionHandlerType: CustomRepetition,
{
    fn new(
        current_tasks: &'srh mut Vec<ScheduledTask<TaskType>>,
        repetition_handler: RepetitionHandlerType,
    ) -> Self {
        Self {
            current_tasks,
            removed_tasks: Vec::new(),
            repetition_handler,
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
                RepetitionType::Yearly(_) => RepetitionHelpers::update_yearly(&now, &mut task.date),
                RepetitionType::ConstGap { gap, count: _ } => {
                    RepetitionHelpers::update_const_gap(&now, &mut task.date, *gap);
                }
                RepetitionType::Custom => {
                    if let Some(new_date) = self.repetition_handler.update_date(&now, &task.date) {
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
                    if let Some(new_date) = self.repetition_handler.update_date(&now, &task.date) {
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

    custom_repetition: CustomRepetitionType,
}

impl<TaskType> BlockingScheduler<TaskType, NoCustomRepetition>
where
    TaskType: Eq + Default,
{
    pub fn new(
        scheduled_tasks: HashMap<String, Vec<ScheduledTask<TaskType>>>,
        mut removed_tasks: HashMap<String, Vec<ScheduledTask<TaskType>>>,
    ) -> Self {
        SchedulerHelper::format_removed_tasks::<TaskType, NoCustomRepetition>(
            &scheduled_tasks,
            &mut removed_tasks,
        );
        Self {
            scheduled_tasks,
            removed_tasks,
            custom_repetition: NoCustomRepetition,
        }
    }
}

impl<TaskType, CustomRepetitionType> BlockingScheduler<TaskType, CustomRepetitionType>
where
    TaskType: Eq + Default,
    CustomRepetitionType: CustomRepetition + Clone,
{
    fn new_with_custom_repetition(
        scheduled_tasks: HashMap<String, Vec<ScheduledTask<TaskType>>>,
        mut removed_tasks: HashMap<String, Vec<ScheduledTask<TaskType>>>,
        custom_repetition: CustomRepetitionType,
    ) -> Self {
        SchedulerHelper::format_removed_tasks::<TaskType, CustomRepetitionType>(
            &scheduled_tasks,
            &mut removed_tasks,
        );
        Self {
            scheduled_tasks,
            removed_tasks,
            custom_repetition,
        }
    }

    pub fn start(&mut self, mode: &str, f: fn(&TaskType)) -> Result<(), String> {
        let mut reading_handler = SchedulerReadingHandler::new(
            self.scheduled_tasks
                .get_mut(mode)
                .ok_or(format!("Couldn't find the requested mode : {}", mode))?,
            self.custom_repetition.clone(),
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
where
    TaskType: Eq + Default,
{
    pub fn new(
        scheduled_tasks: HashMap<String, Vec<ScheduledTask<TaskType>>>,
        removed_tasks: HashMap<String, Vec<ScheduledTask<TaskType>>>,
    ) -> Self {
        Self {
            scheduler: BlockingScheduler::new(scheduled_tasks, removed_tasks),
            scope_thread_handlers: vec![],
            thread_handlers: vec![],
        }
    }
}

impl<'ps, TaskType, CustomRepetitionType> ParallelScheduler<'ps, TaskType, CustomRepetitionType>
where
    TaskType: Eq + Default + Send + Sync + Clone,
    CustomRepetitionType: CustomRepetition + Clone + Send + Sync,
{
    pub fn new_with_custom_repetition(
        scheduled_tasks: HashMap<String, Vec<ScheduledTask<TaskType>>>,
        removed_tasks: HashMap<String, Vec<ScheduledTask<TaskType>>>,
        custom_repetition: CustomRepetitionType,
    ) -> Self {
        Self {
            scheduler: BlockingScheduler::new_with_custom_repetition(
                scheduled_tasks,
                removed_tasks,
                custom_repetition,
            ),
            scope_thread_handlers: vec![],
            thread_handlers: vec![],
        }
    }

    pub fn start(&mut self, mode: String, f: fn(&TaskType)) -> std::io::Result<()>
    where
        TaskType: 'static,
        CustomRepetitionType: 'static,
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
        CustomRepetitionType: 'ps,
    {
        let mut scheduler = self.scheduler.clone();
        thread::scope(|scope| {
            scope.spawn(move || scheduler.start(mode.as_str(), f));
        });
        Ok(())
    }
}
