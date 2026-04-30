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
/// Carries the target registry and host platform flag so the compiler can lower
/// the same logical item differently per target (hook script selection, etc.).
pub struct CompileContext {
    /// Available target adapters, keyed by target root name.
    pub target_registry: TargetRegistry,
    /// Whether the host is Windows — used for hook file selection, not content variation.
    pub is_windows: bool,
}

impl CompileContext {
    /// Construct a `CompileContext` with the default built-in target registry
    /// and the current host platform.
    pub fn new() -> Self {
        Self {
            target_registry: TargetRegistry::new(),
            is_windows: cfg!(windows),
        }
    }
}

impl Default for CompileContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Compiler plan output — seam for Phase 2+ when the compiler produces
/// per-target output records. Currently unused; exists as a forward declaration
/// so the type is available when the pipeline is wired through target adapters.
///
/// Future: per-target output records, lossiness annotations.
pub struct CompilePlan {
    /// Target-aware translation context for this run.
    pub context: CompileContext,
    /// The concrete sync plan produced by the diff+plan phase.
    pub sync_plan: SyncPlan,
}
