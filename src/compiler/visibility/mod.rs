use crate::diagnostic::DiagnosticCollector;

pub(crate) fn default_visibility() -> String {
    "local".to_string()
}

/// Whether an effectful package item crosses a package boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Local,
    Exported,
}

impl Visibility {
    pub(crate) fn parse(
        value: &str,
        item_kind: &str,
        item_name: &str,
        source_name: &str,
        diag: &mut DiagnosticCollector,
    ) -> Self {
        match value {
            "exported" => Self::Exported,
            "local" => Self::Local,
            unknown => {
                diag.warn(
                    "unknown-visibility",
                    format!(
                        "{item_kind} `{item_name}` from source `{source_name}` has unknown \
                         visibility `{unknown}`; treating it as `local`"
                    ),
                );
                Self::Local
            }
        }
    }
}
