use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Breakpoint {
    pub id: u32,
    pub file: PathBuf,
    pub line: u32,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructMember {
    pub name: String,
    pub type_name: String,
    pub value: String,
    pub num_children: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Variable {
    pub name: String,
    pub value: String,
    pub type_name: String,
    /// 構造体型変数のメンバ一覧（-var-list-children で取得、非構造体は None）
    pub members: Option<Vec<StructMember>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frame {
    pub level: u32,
    pub addr: String,
    pub func: String,
    pub file: Option<PathBuf>,
    pub line: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct DebuggerState {
    pub file: Option<PathBuf>,
    pub line: Option<u32>,
    pub breakpoints: Vec<Breakpoint>,
    pub variables: Vec<Variable>,
    pub call_stack: Vec<Frame>,
}

impl DebuggerState {
    pub fn new() -> Self {
        Self {
            file: None,
            line: None,
            breakpoints: Vec::new(),
            variables: Vec::new(),
            call_stack: Vec::new(),
        }
    }
}
