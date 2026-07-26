use crate::agent::context::engine_state::EngineState;
use super::context_engine::ContextEngine;
pub struct ContextCompressor {
    pub engine_state: EngineState,
}

impl ContextEngine for ContextCompressor {

}