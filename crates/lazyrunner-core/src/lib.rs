//! lazyrunner's core: the apps it runs, their output, and the processes behind them. No terminal code here.

pub mod apptypes;
pub mod changelog;
pub mod config;
pub mod emitter;
pub mod ipc;
pub mod json;
pub mod metrics;
pub mod output;
pub mod paths;
pub mod process;
pub mod profile;
pub mod profiletools;
pub mod run;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
