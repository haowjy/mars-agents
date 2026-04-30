/// Translation context for a single compilation run.
///
/// Constructed once at the start of `compiler::compile()` and threaded
/// through the pipeline. Carries target-aware state so the compiler can
/// lower the same logical item differently per target (e.g., hook script
/// selection, lossiness classification). Reader stages are target-neutral
/// and never see this type.
use crate::sync::plan::SyncPlan;
use crate::target::TargetRegistry;

/// Context for a single compilation run.
///
/// Carries the target registry and host platform so the compiler can lower
/// the same logical item differently per target.
pub struct CompileContext {
    /// Available target adapters, keyed by target root name.
    pub target_registry: TargetRegistry,
    /// Host platform — used for hook file selection, not content variation.
    pub platform: Platform,
}

impl CompileContext {
    /// Construct a `CompileContext` with the default built-in target registry
    /// and the current host platform.
    pub fn new() -> Self {
        Self {
            target_registry: TargetRegistry::new(),
            platform: Platform::current(),
        }
    }
}

impl Default for CompileContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Output of the compiler's planning phase.
///
/// Bundles the translation context with the concrete sync plan so downstream
/// apply/write stages have both what to do and how to interpret it per target.
///
/// Future: per-target output records, lossiness annotations.
pub struct CompilePlan {
    /// Target-aware translation context for this run.
    pub context: CompileContext,
    /// The concrete sync plan produced by the diff+plan phase.
    pub sync_plan: SyncPlan,
}

/// Host platform — used for hook file selection, not content variation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Unix,
}

impl Platform {
    /// Detect the current host platform at compile time.
    pub fn current() -> Self {
        if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Unix
        }
    }
}
