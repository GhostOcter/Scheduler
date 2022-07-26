use chrono::{DateTime, Datelike, Duration, FixedOffset, Local, TimeZone, Timelike};
use serde::{
    de::{EnumAccess, VariantAccess, Visitor},
    ser::SerializeStructVariant,
    Deserialize, Serialize,
};
use serde_json;
use spin_sleep::{SpinSleeper, SpinStrategy};
use std::cmp::Ordering;
use std::collections::HashMap;

#[derive(PartialEq, Eq, Clone, Debug, Serialize, Deserialize, Default)]
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

#[serde_with::serde_as]
#[derive(PartialEq, Eq, Clone, Debug, Serialize, Deserialize, Default)]
pub enum Repetition {
    #[default]
    Once,
    Weekly(RepetitionCount),
    Monthly(RepetitionCount),
    Yearly(RepetitionCount),
    Custom {
        #[serde_as(as = "serde_with::DurationSeconds<i64>")]
        gap: Duration,
        count: RepetitionCount,
    },
}
#[derive(Deserialize, Serialize, Debug)]
struct SpinSleeperFields {
    native_accuracy_ns: u32,
    spin_strategy: u8,
}
// You need to know that the
#[derive(PartialEq, Eq, Clone, Debug, Default)]
pub enum SleepType {
    #[default]
    // Accurate to the second => Use std::thread::sleep =>
    Native,
    // Accurate to the millisecond => Use spin sleep which require more ressoruces to work
    SpinSleep(SpinSleeper),
}

impl Serialize for SleepType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match &self {
            Self::Native => serializer.serialize_unit_variant("SleepType", 0, "Native"),
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
#[derive(PartialEq, Eq, Clone, Debug, Serialize, Deserialize)]
pub struct ActionPlanned<T> {
    pub action: T,
    pub date: DateTime<FixedOffset>,
    pub repetition: Repetition,
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
    fn new(date: DateTime<FixedOffset>, action: T, repetition: Repetition, sleep_type: SleepType) -> Self {
        Self {
            date, 
            action,
            repetition,
            sleep_type
        }
    }
}
pub struct PlannerReadingHandler<'pr, T> {
    current_actions: &'pr mut Vec<ActionPlanned<T>>,
    removed_actions: Vec<ActionPlanned<T>>,
}
impl<'pr, T> PlannerReadingHandler<'pr, T> {
    fn new(current_actions: &'pr mut Vec<ActionPlanned<T>>) -> Self {
        Self {
            current_actions,
            removed_actions: Vec::new(),
        }
    }
    fn get_current_action(&self) -> Option<&ActionPlanned<T>> {
        if !self.current_actions.is_empty() {
            // I do this to get the ref not mutable since the mut ref is able to call Ref::from_mut_ref() (or something like that)
            unsafe { Some(self.current_actions.get(0).unwrap_unchecked()) }
        } else {
            None
        }
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
                Repetition::Once => {
                    self.remove_action(i);
                }
                Repetition::Weekly(_) => {
                    // Important to keep: weekday, time
                    let diff = now - action.date;
                    action.date =
                        action.date + Duration::days(diff.num_days() - diff.num_days() % 7 + 7);
                    // This permits to get the next requestad weekday coming (Mon, Tue, etc...)
                }
                Repetition::Monthly(_) => {
                    // Important to keep: month's day, time
                    if (action.date.month0() + 1) % 12 != 2 {
                        // Not February
                        let diff = now - action.date;
                        action.date =
                            action.date + Duration::days(diff.num_days() - diff.num_days() % 7 + 7);
                    } else { // February
                    }
                }
                Repetition::Yearly(_) => {
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
                Repetition::Custom { gap, count: _ } => {
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
                Repetition::Once => {
                    self.remove_action(i);
                }
                Repetition::Weekly(count) => {
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
                Repetition::Monthly(count) => {
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
                Repetition::Yearly(count) => {
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
                Repetition::Custom { gap, count } => {
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
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Planner<T> {
    pub planned_actions: HashMap<String, Vec<ActionPlanned<T>>>,
    pub removed_actions: HashMap<String, Vec<ActionPlanned<T>>>, //TODO: Create a handler for the planning'history ie all the actions that had been removed
}

impl<T> Planner<T>
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
    pub fn into_pretty_json_string(&self) -> Result<String, std::io::Error>
    where
        T: Serialize,
    {
        Ok(serde_json::to_string_pretty(&self)?)
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
    pub fn from_json_string<'de>(
        json_content: &'de mut String,
    ) -> Result<Self, Box<dyn std::error::Error>>
    where
        T: Deserialize<'de>,
        // TODO: DeserializeOwned ??
    {
        let mut planning: Planner<T> = serde_json::from_str(json_content)?;
        Self::format_removed_actions(&planning.planned_actions, &mut planning.removed_actions);
        Ok(planning)
    }

    pub fn start(&mut self, mode: &str, f: fn(&T)) -> Result<(), Box<dyn std::error::Error>> {
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
                    let diff = (action.date - now).to_std()?;
                    match action.sleep_type {
                        SleepType::Native => {
                            std::thread::sleep(diff);
                        }
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
            // This is safe since we applied Self::check_presences_of_all_modes_in_removed_actions when this struct was constructed
            self.removed_actions
                .get_mut(mode)
                .unwrap_unchecked()
                .append(&mut reading_handler.removed_actions);
        }
        Ok(())
    }
}
