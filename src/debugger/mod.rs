use anyhow::Result;
use std::path::Path;

pub mod gdb;
pub mod types;

pub use types::{Breakpoint, DebuggerState, StructMember, Variable};

#[allow(dead_code)]
pub trait Debugger: Send {
    fn next(&mut self) -> Result<()>;
    fn step(&mut self) -> Result<()>;
    fn finish(&mut self) -> Result<()>;
    fn cont(&mut self) -> Result<()>;
    fn toggle_breakpoint(&mut self, file: &Path, line: u32) -> Result<()>;
    fn get_state(&self) -> &DebuggerState;
}
