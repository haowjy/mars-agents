use mars_schema::version::Constraint;
use semver::Version;

/// Result of comparing two version constraints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompatibilityResult {
    /// Both constraints resolve to the same version.
    Compatible,
    /// Constraints might resolve differently (Latest vs pinned).
    PotentiallyConflicting,
    /// Constraints cannot resolve to the same version.
    Conflicting,
}

/// Check if this constraint is compatible with another.
///
/// Matrix:
/// - Latest + Latest => Compatible
/// - Semver(same) + Semver(same) => Compatible
/// - RefPin(same) + RefPin(same) => Compatible
/// - Latest + Semver/RefPin => PotentiallyConflicting
/// - Different Semver/RefPin => Conflicting
/// - Semver + RefPin => Conflicting
pub fn compatible_with(this: &Constraint, other: &Constraint) -> CompatibilityResult {
    use CompatibilityResult::{Compatible, Conflicting, PotentiallyConflicting};
    use Constraint::{Latest, RefPin, Semver};

    match (this, other) {
        (Latest, Latest) => Compatible,
        (Latest, Semver(_) | RefPin(_)) | (Semver(_) | RefPin(_), Latest) => PotentiallyConflicting,
        (Semver(lhs), Semver(rhs)) => {
            if lhs == rhs {
                Compatible
            } else {
                Conflicting
            }
        }
        (RefPin(lhs), RefPin(rhs)) => {
            if lhs == rhs {
                Compatible
            } else {
                Conflicting
            }
        }
        (Semver(_), RefPin(_)) | (RefPin(_), Semver(_)) => Conflicting,
    }
}

/// Check compatibility against a concrete resolved version.
///
/// This is stricter than pure syntactic comparison for semver constraints:
/// two different semver expressions are compatible when both accept the
/// already-resolved concrete version.
pub fn compatible_with_resolved(
    this: &Constraint,
    other: &Constraint,
    resolved_version: Option<&Version>,
) -> CompatibilityResult {
    use CompatibilityResult::{Compatible, Conflicting, PotentiallyConflicting};
    use Constraint::{Latest, Semver};

    match (this, other) {
        (Semver(lhs), Semver(rhs)) => {
            if lhs == rhs {
                Compatible
            } else if let Some(version) = resolved_version {
                if lhs.matches(version) && rhs.matches(version) {
                    Compatible
                } else {
                    Conflicting
                }
            } else {
                Conflicting
            }
        }
        // Latest vs Semver: if the resolved version satisfies the semver
        // constraint, they agree on the same concrete version — no drift.
        (Latest, Semver(req)) | (Semver(req), Latest) => {
            if let Some(version) = resolved_version {
                if req.matches(version) {
                    Compatible
                } else {
                    PotentiallyConflicting
                }
            } else {
                PotentiallyConflicting
            }
        }
        _ => compatible_with(this, other),
    }
}

#[cfg(test)]
mod tests {
    use super::{CompatibilityResult, compatible_with, compatible_with_resolved};
    use mars_schema::version::Constraint;
    use semver::Version;

    fn semver(req: &str) -> Constraint {
        Constraint::Semver(req.parse().expect("valid semver requirement"))
    }

    #[test]
    fn latest_with_latest_is_compatible() {
        assert_eq!(
            compatible_with(&Constraint::Latest, &Constraint::Latest),
            CompatibilityResult::Compatible
        );
    }

    #[test]
    fn same_semver_is_compatible() {
        assert_eq!(
            compatible_with(&semver("^1.2"), &semver("^1.2")),
            CompatibilityResult::Compatible
        );
    }

    #[test]
    fn same_ref_pin_is_compatible() {
        assert_eq!(
            compatible_with(
                &Constraint::RefPin("main".into()),
                &Constraint::RefPin("main".into())
            ),
            CompatibilityResult::Compatible
        );
    }

    #[test]
    fn latest_with_semver_is_potentially_conflicting() {
        assert_eq!(
            compatible_with(&Constraint::Latest, &semver(">=1.0.0")),
            CompatibilityResult::PotentiallyConflicting
        );
    }

    #[test]
    fn latest_with_ref_pin_is_potentially_conflicting() {
        assert_eq!(
            compatible_with(&Constraint::Latest, &Constraint::RefPin("main".into())),
            CompatibilityResult::PotentiallyConflicting
        );
    }

    #[test]
    fn different_semver_is_conflicting() {
        assert_eq!(
            compatible_with(&semver("^1.0"), &semver("^2.0")),
            CompatibilityResult::Conflicting
        );
    }

    #[test]
    fn different_ref_pin_is_conflicting() {
        assert_eq!(
            compatible_with(
                &Constraint::RefPin("main".into()),
                &Constraint::RefPin("release".into())
            ),
            CompatibilityResult::Conflicting
        );
    }

    #[test]
    fn semver_with_ref_pin_is_conflicting() {
        assert_eq!(
            compatible_with(&semver("^1.0"), &Constraint::RefPin("main".into())),
            CompatibilityResult::Conflicting
        );
    }

    #[test]
    fn equivalent_semver_syntax_is_compatible_for_resolved_version() {
        let resolved = Version::new(1, 4, 2);
        assert_eq!(
            compatible_with_resolved(&semver("^1.0"), &semver(">=1.0.0, <2.0.0"), Some(&resolved)),
            CompatibilityResult::Compatible
        );
    }

    #[test]
    fn incompatible_semver_syntax_is_conflicting_for_resolved_version() {
        let resolved = Version::new(2, 0, 0);
        assert_eq!(
            compatible_with_resolved(&semver("^1.0"), &semver(">=1.0.0, <2.0.0"), Some(&resolved)),
            CompatibilityResult::Conflicting
        );
    }

    #[test]
    fn latest_with_semver_compatible_when_resolved_matches() {
        let resolved = Version::new(0, 2, 1);
        assert_eq!(
            compatible_with_resolved(&Constraint::Latest, &semver("=0.2.1"), Some(&resolved)),
            CompatibilityResult::Compatible
        );
        // Symmetric
        assert_eq!(
            compatible_with_resolved(&semver("=0.2.1"), &Constraint::Latest, Some(&resolved)),
            CompatibilityResult::Compatible
        );
    }

    #[test]
    fn latest_with_semver_potentially_conflicting_when_resolved_mismatches() {
        let resolved = Version::new(0, 3, 0);
        assert_eq!(
            compatible_with_resolved(&Constraint::Latest, &semver("=0.2.1"), Some(&resolved)),
            CompatibilityResult::PotentiallyConflicting
        );
    }

    #[test]
    fn latest_with_semver_potentially_conflicting_without_resolved() {
        assert_eq!(
            compatible_with_resolved(&Constraint::Latest, &semver("=0.2.1"), None),
            CompatibilityResult::PotentiallyConflicting
        );
    }
}
