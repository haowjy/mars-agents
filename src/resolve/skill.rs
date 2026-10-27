use mars_schema::version::Constraint;
use std::collections::{HashMap, HashSet, VecDeque};
#[cfg(test)]
use std::path::{Path, PathBuf};

use indexmap::IndexMap;

#[cfg(test)]
use crate::discover;
use crate::error::{MarsError, ResolutionError};
use crate::types::SourceName;
use crate::validate;
use mars_schema::item::ItemKind;
use mars_schema::names::ItemName;

use super::package::RegisteredPackage;
use super::types::PendingItem;

#[derive(Debug, Clone)]
struct ResolvedSkill {
    package: SourceName,
}

struct SkillResolver<'a> {
    registry: &'a IndexMap<SourceName, RegisteredPackage>,
}

impl<'a> SkillResolver<'a> {
    fn resolve(
        &self,
        skill: &ItemName,
        requester_package: &SourceName,
    ) -> Result<ResolvedSkill, ResolutionError> {
        if self
            .registry
            .get(requester_package)
            .is_some_and(|package| package.has_skill(skill))
        {
            return Ok(ResolvedSkill {
                package: requester_package.clone(),
            });
        }

        let closure_order = self.dependency_closure_order(requester_package);
        let closure_set: HashSet<SourceName> = closure_order.iter().cloned().collect();

        for package_name in &closure_order {
            if self
                .registry
                .get(package_name)
                .is_some_and(|package| package.has_skill(skill))
            {
                return Ok(ResolvedSkill {
                    package: package_name.clone(),
                });
            }
        }

        for (package_name, package) in self.registry {
            if package_name == requester_package || closure_set.contains(package_name) {
                continue;
            }
            if !package.has_skill(skill) {
                continue;
            }
            return Ok(ResolvedSkill {
                package: package_name.clone(),
            });
        }

        let mut searched: Vec<String> = self.registry.keys().map(ToString::to_string).collect();
        searched.sort();
        Err(ResolutionError::SkillNotFound {
            skill: skill.to_string(),
            required_by: requester_package.to_string(),
            searched,
            hint: None,
        })
    }

    fn dependency_closure_order(&self, requester_package: &SourceName) -> Vec<SourceName> {
        let Some(requester) = self.registry.get(requester_package) else {
            return Vec::new();
        };

        // Breadth-first traversal keeps direct deps ahead of transitive deps.
        let mut seen: HashSet<SourceName> = HashSet::from([requester_package.clone()]);
        let mut queue: VecDeque<SourceName> = VecDeque::new();
        let mut order = Vec::new();

        for dep in &requester.node.deps {
            if self.registry.contains_key(dep) && seen.insert(dep.clone()) {
                queue.push_back(dep.clone());
            }
        }

        while let Some(package_name) = queue.pop_front() {
            order.push(package_name.clone());

            if let Some(package) = self.registry.get(&package_name) {
                for dep in &package.node.deps {
                    if self.registry.contains_key(dep) && seen.insert(dep.clone()) {
                        queue.push_back(dep.clone());
                    }
                }
            }
        }

        order
    }
}

pub(crate) fn parse_pending_item_skill_deps(
    pending_item: &PendingItem,
    package: &RegisteredPackage,
) -> Result<Vec<ItemName>, MarsError> {
    if !matches!(pending_item.kind, ItemKind::Agent | ItemKind::Skill) {
        return Ok(Vec::new());
    }

    let Some(_discovered) = package.item(pending_item.kind, &pending_item.item) else {
        return Ok(Vec::new());
    };
    let skill_deps = if let Some(prepared) = &package.node.prepared {
        prepared
            .definitions
            .iter()
            .find(|definition| {
                definition.slug == pending_item.item.as_ref()
                    && ItemKind::from(definition.kind) == pending_item.kind
            })
            .map(|definition| {
                validate::extract_skills_from_content(&String::from_utf8_lossy(
                    &definition.normalized_bytes,
                ))
            })
            .unwrap_or_default()
    } else {
        #[cfg(test)]
        {
            let item_path =
                discovered_item_markdown_path(&package.node.rooted_ref.package_root, _discovered);
            validate::parse_item_skill_deps(&item_path)?
        }
        #[cfg(not(test))]
        {
            unreachable!("production resolver nodes retain prepared packages")
        }
    };
    Ok(skill_deps.into_iter().map(ItemName::from).collect())
}

#[cfg(test)]
pub(crate) fn discovered_item_markdown_path(
    package_root: &Path,
    item: &discover::DiscoveredItem,
) -> PathBuf {
    match item.id.kind {
        ItemKind::Agent | ItemKind::McpServer => package_root.join(&item.source_path),
        ItemKind::BootstrapDoc => package_root.join(&item.source_path).join("BOOTSTRAP.md"),
        ItemKind::Hook => package_root.join(&item.source_path).join("hook.toml"),
        ItemKind::Skill => {
            if item.source_path == Path::new(".") {
                package_root.join("SKILL.md")
            } else {
                package_root.join(&item.source_path).join("SKILL.md")
            }
        }
    }
}

/// Resolve one skill ref of `requester` to the package item that supplies it.
///
/// `Ok(None)` defers a ref `x__p` to provider `p` when the requesting package declares `p` in
/// `uses-providers` and no package ships the skill: nothing is pulled and nothing is reported
/// here, because sync's validator is the one reporter of what is installed. The answer depends
/// on the package alone, never on what this machine has provided, so a package resolves the
/// same everywhere. Any other missing skill is `SkillNotFound`; a name shaped like a provider's
/// carries the conditional `uses-providers` hint, because no package declared that provider.
pub(crate) fn resolve_skill_ref(
    skill: &ItemName,
    requester: &PendingItem,
    registry: &IndexMap<SourceName, RegisteredPackage>,
    constraints: &HashMap<SourceName, Vec<(String, Constraint)>>,
) -> Result<Option<PendingItem>, MarsError> {
    let required_by = format!("{}/{}", requester.package, requester.item);
    let resolver = SkillResolver { registry };
    let resolved_skill = match resolver.resolve(skill, &requester.package) {
        Ok(resolved_skill) => resolved_skill,
        Err(ResolutionError::SkillNotFound {
            skill: missing_skill,
            searched,
            ..
        }) => {
            let declared = registry
                .get(&requester.package)
                .and_then(|package| package.node.manifest.as_ref())
                .map_or(&[][..], |manifest| {
                    manifest.package.uses_providers.as_slice()
                });
            // A declared provider resolves wherever it supplies the skill; the resolver only
            // needs the package's answer, never what this machine installed.
            if validate::deferred_provider(skill.as_str(), Some(declared)).is_some() {
                return Ok(None);
            }
            let hint = mars_schema::provider::provider_of(skill.as_str())
                .map(validate::undeclared_provider_hint);
            return Err(ResolutionError::SkillNotFound {
                skill: missing_skill,
                required_by,
                searched,
                hint,
            }
            .into());
        }
        Err(err) => return Err(err.into()),
    };

    let package =
        registry
            .get(&resolved_skill.package)
            .ok_or_else(|| ResolutionError::SourceNotFound {
                name: resolved_skill.package.to_string(),
            })?;
    let constraint = primary_package_constraint(constraints, &resolved_skill.package)
        .unwrap_or(&package.constraint)
        .clone();
    Ok(Some(PendingItem {
        package: resolved_skill.package,
        item: skill.clone(),
        kind: ItemKind::Skill,
        constraint,
        required_by,
        is_local: package.is_local,
    }))
}

pub(crate) fn primary_package_constraint<'a>(
    constraints: &'a HashMap<SourceName, Vec<(String, Constraint)>>,
    package: &SourceName,
) -> Option<&'a Constraint> {
    constraints
        .get(package)
        .and_then(|entries| entries.first().map(|(_, constraint)| constraint))
}
