use crate::build::bundle::{LaunchActions, LaunchBundle, RuntimeContext};
use crate::error::MarsError;

#[allow(dead_code)]
pub fn project(
    _bundle: &LaunchBundle,
    _context: &RuntimeContext,
) -> Result<LaunchActions, MarsError> {
    Err(MarsError::InvalidRequest {
        message: "launch_actions projection not implemented for this harness yet".to_string(),
    })
}

#[allow(dead_code)]
pub fn project_subprocess(
    _bundle: &LaunchBundle,
    _context: &RuntimeContext,
) -> Result<LaunchActions, MarsError> {
    Err(MarsError::InvalidRequest {
        message: "launch_actions projection not implemented for this harness yet".to_string(),
    })
}

#[allow(dead_code)]
pub fn project_streaming(
    _bundle: &LaunchBundle,
    _context: &RuntimeContext,
) -> Result<LaunchActions, MarsError> {
    Err(MarsError::InvalidRequest {
        message: "launch_actions projection not implemented for this harness yet".to_string(),
    })
}
