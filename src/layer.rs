//! The content layers a project's sync and a launch read, in rank order: the project's own
//! packages, the user level, and a host's bundle. This table is the one place a layer's
//! properties live; sync injection (`sync::load_config`), the claim rule ([`yielded`] at sync,
//! `content::ContentRoots::lookup` at launch), the machine-local split (`lock::machine`) and
//! target projection ([`offered_to`]) all read it.
//!
//! A layer other than the project's is recognized by its reserved source prefix: every source
//! it brings into a project's graph is named `<prefix><name>`, and no manifest may name a
//! dependency with one (`mars_schema::dependency::RESERVED_PREFIXES`).

use std::collections::{HashMap, HashSet};
use std::hash::Hash;

/// A content layer. The derive order is the rank: a lower layer is claimed first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Layer {
    /// The project's dependencies and `.mars-src` (rank 1).
    Project,
    /// The user level, pinned to its own lock (rank 2).
    User,
    /// A host's bundle, `_provided-<host>` (rank 3).
    Host,
}

pub(crate) use mars_schema::dependency::{HOST_PREFIX, USER_PREFIX};
/// The user level's own items (`~/.mars/.mars-src`) in a project.
pub(crate) const USER_SELF: &str = "_user-self";

struct Row {
    layer: Layer,
    /// `None` for the project, whose sources are named as `mars.toml` names them.
    prefix: Option<&'static str>,
    /// Whether the layer's content goes to every managed target, or only to those that take a
    /// host's content (`harness::registry::takes_provided`).
    every_target: bool,
}

const LAYERS: [Row; 3] = [
    Row {
        layer: Layer::Project,
        prefix: None,
        every_target: true,
    },
    Row {
        layer: Layer::User,
        prefix: Some(USER_PREFIX),
        every_target: true,
    },
    Row {
        layer: Layer::Host,
        prefix: Some(HOST_PREFIX),
        every_target: false,
    },
];

impl Layer {
    /// The layer a source belongs to, by its name.
    pub(crate) fn of(source_name: &str) -> Self {
        LAYERS
            .iter()
            .find(|row| {
                row.prefix
                    .is_some_and(|prefix| source_name.starts_with(prefix))
            })
            .map_or(Self::Project, |row| row.layer)
    }

    fn row(self) -> &'static Row {
        LAYERS
            .iter()
            .find(|row| row.layer == self)
            .expect("every layer has a row")
    }

    /// The layer's reserved source prefix; `None` for the project.
    pub(crate) fn prefix(self) -> Option<&'static str> {
        self.row().prefix
    }

    /// Whether this layer's records describe one machine, so they go in `.mars/machine.lock` and
    /// never in the committed `mars.lock`: every layer but the project's.
    pub(crate) fn machine_local(self) -> bool {
        self.prefix().is_some()
    }
}

/// Whether `source_name` came from a layer other than the project's.
pub(crate) fn is_machine_local(source_name: &str) -> bool {
    Layer::of(source_name).machine_local()
}

/// Whether `source_name` is a host's bundle (`_provided-<host>`).
pub(crate) fn is_host(source_name: &str) -> bool {
    Layer::of(source_name) == Layer::Host
}

/// Whether content of `source_name` is offered to `target`.
pub(crate) fn offered_to(source_name: &str, target: &str) -> bool {
    Layer::of(source_name).row().every_target || crate::harness::registry::takes_provided(target)
}

/// The claim rule: for each key, the claims of a layer ranked after the best layer that claims
/// it give way. Returns `(key, layer)` for every claim that yields. Within one layer nothing
/// yields: a collision there renames every side (`sync::target`). Sync keys a claim by the
/// item's name before collision renames, so a name a better layer renamed still blocks the
/// worse layer's copy, as launch lookup does (`content::ContentRoots::lookup`).
pub(crate) fn yielded<K: Eq + Hash + Clone>(
    claims: impl IntoIterator<Item = (K, Layer)>,
) -> HashSet<(K, Layer)> {
    let claims: Vec<(K, Layer)> = claims.into_iter().collect();
    let mut best: HashMap<&K, Layer> = HashMap::new();
    for (key, layer) in &claims {
        best.entry(key)
            .and_modify(|held| *held = (*held).min(*layer))
            .or_insert(*layer);
    }
    claims
        .iter()
        .filter(|(key, layer)| best[key] < *layer)
        .cloned()
        .collect()
}

/// A source for a message to the user: the dependency name for the project's, `<name> (user
/// level)` for the user level's, the host's name (`meridian`) for a host's.
pub(crate) fn source_label(source_name: &str) -> String {
    match Layer::of(source_name) {
        Layer::Project => source_name.to_string(),
        Layer::User if source_name == USER_SELF => "user level".to_string(),
        Layer::User => format!("{} (user level)", &source_name[USER_PREFIX.len()..]),
        Layer::Host => source_name[HOST_PREFIX.len()..].to_string(),
    }
}
