use crate::error::MarsError;

use super::TreeProfile;

pub(super) fn profile() -> Result<&'static TreeProfile, MarsError> {
    Err(MarsError::InvalidRequest {
        message: "the OpenCode plugin tree is not implemented yet".to_string(),
    })
}
