pub mod manifest;
mod common;
pub mod diffing;
pub mod zstddiff;
mod hash;
pub mod applying;
mod threading;
pub mod upgrade;
pub mod verify;
pub mod reporting;

pub use crate::threading::set_num_threads;
pub use crate::common::FoldiffCfg;