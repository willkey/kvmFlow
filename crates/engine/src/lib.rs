#![forbid(unsafe_code)]

//! Orchestrates the V2 runtime actor and its effects.

mod actor;

pub use actor::{
    EngineCommand, EngineConfig, EngineError, EngineHandle, spawn_engine, spawn_engine_with_kvm,
    spawn_engine_with_runtime,
};
