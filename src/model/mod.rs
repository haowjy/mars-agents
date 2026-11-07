use crate::local_source::LocalDiscoveredItem;
use crate::sync::ResolvedState;

/// The compiler owns target paths and plans; this carries resolved sources and the prior lock.
pub struct ReaderIr {
    pub resolved: ResolvedState,
    pub local_items: Vec<LocalDiscoveredItem>,
}
