//! #Planner
//! A Rust crate that allows code to be called in a scheduled way
//!
//!  
//! #Example :
//! ```
//!```
pub mod repetitions;
pub mod schedulers;
pub mod sleeptype;
pub mod prelude {
    pub use super::repetitions::*;
    pub use super::schedulers::{BlockingScheduler, ParallelScheduler, ScheduledTask};
    pub use super::sleeptype::SleepType;
}
