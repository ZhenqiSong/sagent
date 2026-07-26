
pub struct EngineState{
    pub context_length: u32,
}

impl Default for EngineState {
    fn default() -> Self {
        Self { context_length: 0 }
    }
}
