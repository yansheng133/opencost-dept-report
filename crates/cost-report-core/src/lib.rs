//! Department cost allocation from OpenCost.
//!
//! Pure logic only: no network, no Kubernetes, no clock except where a
//! module says so. The binary (`cost-report`) does the I/O and calls in.
//!
//! * [`billing`]  — rate card, rate check, reconciliation, billing gate
//! * [`seals`]    — sealed days and monthly periods
//! * [`report`]   — the department report from OpenCost's allocations
//! * [`coverage`] — measured share of a window, gaps, scrape targets
//! * [`series`]   — usage over time
//! * [`steps`]    — allowed sampling steps and the query budget
//! * [`storage`]  — claimed vs used storage
//! * [`scope`]    — cutting responses to the departments a caller may see
//! * [`snapshot`] — declared-request snapshots (second allocation basis)
//! * [`volume`]   — measuring local-path PVC use on a node

pub mod billing;
pub mod coverage;
pub mod num;
pub mod report;
pub mod scope;
pub mod seals;
pub mod series;
pub mod snapshot;
pub mod steps;
pub mod storage;
pub mod time;
pub mod volume;
