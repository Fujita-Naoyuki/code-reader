use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Workspace {
    pub roots: Vec<PathBuf>,
}

impl Workspace {
    pub fn empty() -> Self {
        Self { roots: Vec::new() }
    }
}
