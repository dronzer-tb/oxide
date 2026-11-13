//! Tag files (`data/<namespace>/tags/<kind>/**.json`): named sets of blocks, fluids or biomes.
//!
//! Decoration cannot run without them. Vanilla's features ask "is this `#minecraft:dirt`",
//! "is this `#minecraft:replaceable_by_trees`" at almost every block they touch, and the answer
//! lives in these files rather than in code -- so, like the rest of worldgen, it is loaded from
//! the extracted game data and never written from memory.
//!
//! A tag file lists ids and `#`-prefixed references to other tags. [`TagSet`] is the resolved
//! form: every tag flattened to the plain ids it contains.

use crate::error::{DatapackError, Result};
use crate::registry::load_registry_at;
use oxide_core::ResourceLocation;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::str::FromStr;

#[derive(Debug, Deserialize)]
struct TagFile {
    #[serde(default)]
    replace: bool,
    values: Vec<TagEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum TagEntry {
    Plain(String),
    Optional {
        id: String,
        #[serde(default = "default_required")]
        required: bool,
    },
}

fn default_required() -> bool {
    true
}

impl TagEntry {
    fn parts(&self) -> (&str, bool) {
        match self {
            TagEntry::Plain(id) => (id, true),
            TagEntry::Optional { id, required } => (id, *required),
        }
    }
}

/// Every tag of one kind (`block`, `fluid`, `worldgen/biome`), flattened to the ids it holds.
///
/// Each tag is kept twice: as a set for membership, and as a list in vanilla's order for the
/// features that draw a random member (`Registry.getRandomElementOf` indexes the tag's list).
/// That order is `TagLoader`'s: entries in file and pack order, a nested tag expanded in place,
/// each id kept at its first appearance.
#[derive(Debug, Default, Clone)]
pub struct TagSet {
    tags: HashMap<ResourceLocation, HashSet<ResourceLocation>>,
    ordered: HashMap<ResourceLocation, Vec<ResourceLocation>>,
}

impl TagSet {
    /// Whether `id` is a member of `tag`. An unknown tag has no members.
    pub fn contains(&self, tag: &ResourceLocation, id: &ResourceLocation) -> bool {
        self.tags.get(tag).is_some_and(|members| members.contains(id))
    }

    pub fn get(&self, tag: &ResourceLocation) -> Option<&HashSet<ResourceLocation>> {
        self.tags.get(tag)
    }

    /// The members of `tag` in vanilla's order; `None` for a tag the data does not define.
    pub fn ordered(&self, tag: &ResourceLocation) -> Option<&[ResourceLocation]> {
        self.ordered.get(tag).map(Vec::as_slice)
    }

    pub fn len(&self) -> usize {
        self.tags.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
    }
}

/// Loads and resolves the tags under `tags/<kind>` across `pack_roots`, in stack order.
///
/// A later pack's file for the same tag appends to the earlier one's entries unless it sets
/// `"replace": true`, as vanilla's tag loader does. A pack tree with no such folder yields an
/// empty set rather than an error: reference trees extracted before decoration existed have no
/// tags, and terrain-only generation does not need them.
pub fn load_tags(pack_roots: &[&Path], kind: &str) -> Result<TagSet> {
    let subpath = Path::new("tags").join(kind);
    let mut raw: HashMap<ResourceLocation, Vec<(String, bool)>> = HashMap::new();
    for root in pack_roots {
        if !root.join("data").is_dir() {
            continue;
        }
        let files = load_registry_at::<TagFile>(root, &subpath, "tag")?;
        for (id, file) in files.iter() {
            let entries = raw.entry(id.clone()).or_default();
            if file.replace {
                entries.clear();
            }
            entries.extend(file.values.iter().map(|e| {
                let (id, required) = e.parts();
                (id.to_string(), required)
            }));
        }
    }

    let mut resolved = HashMap::new();
    let mut ids: Vec<&ResourceLocation> = raw.keys().collect();
    ids.sort_by_key(|id| id.to_string());
    for id in ids {
        let mut stack = Vec::new();
        resolve(id, &raw, &mut resolved, &mut stack)?;
    }
    let tags = resolved
        .iter()
        .map(|(id, members)| (id.clone(), members.iter().cloned().collect()))
        .collect();
    Ok(TagSet { tags, ordered: resolved })
}

/// Resolves `tag` into `resolved` as an ordered, duplicate-free list (see [`TagSet`]).
fn resolve(
    tag: &ResourceLocation,
    raw: &HashMap<ResourceLocation, Vec<(String, bool)>>,
    resolved: &mut HashMap<ResourceLocation, Vec<ResourceLocation>>,
    stack: &mut Vec<ResourceLocation>,
) -> Result<()> {
    if resolved.contains_key(tag) {
        return Ok(());
    }
    if stack.contains(tag) {
        let mut cycle: Vec<String> = stack.iter().map(ToString::to_string).collect();
        cycle.push(tag.to_string());
        return Err(DatapackError::Cycle {
            registry: "tag",
            cycle: cycle.join(" -> "),
        });
    }
    stack.push(tag.clone());
    let mut seen = HashSet::new();
    let mut members = Vec::new();
    for (entry, required) in &raw[tag] {
        if let Some(reference) = entry.strip_prefix('#') {
            let target = parse_id(reference)?;
            if !raw.contains_key(&target) {
                if *required {
                    return Err(DatapackError::DanglingReference {
                        registry: "tag",
                        referrer: tag.to_string(),
                        kind: "tag",
                        target: target.to_string(),
                    });
                }
                continue;
            }
            resolve(&target, raw, resolved, stack)?;
            for id in &resolved[&target] {
                if seen.insert(id.clone()) {
                    members.push(id.clone());
                }
            }
        } else {
            let id = parse_id(entry)?;
            if seen.insert(id.clone()) {
                members.push(id);
            }
        }
    }
    stack.pop();
    resolved.insert(tag.clone(), members);
    Ok(())
}

fn parse_id(text: &str) -> Result<ResourceLocation> {
    ResourceLocation::from_str(text).map_err(|_| {
        DatapackError::InvalidResourceLocation(text.to_string(), std::path::PathBuf::from("tags"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join("data/minecraft/tags/block").join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    fn rl(path: &str) -> ResourceLocation {
        ResourceLocation::minecraft(path)
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("oxide-tags-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn nested_tags_flatten_and_packs_stack() {
        let base = temp("base");
        let over = temp("over");
        write(&base, "dirt.json", r#"{"values":["minecraft:dirt","minecraft:coarse_dirt"]}"#);
        write(&base, "soil.json", r##"{"values":["#minecraft:dirt","minecraft:mud"]}"##);
        write(&base, "mineable/axe.json", r#"{"values":["minecraft:oak_log"]}"#);
        write(&over, "dirt.json", r#"{"values":["minecraft:podzol"]}"#);
        write(&over, "soil.json", r#"{"replace":true,"values":["minecraft:clay"]}"#);

        let tags = load_tags(&[&base, &over], "block").unwrap();
        assert!(tags.contains(&rl("dirt"), &rl("coarse_dirt")));
        assert!(tags.contains(&rl("dirt"), &rl("podzol")), "a later pack appends");
        assert!(tags.contains(&rl("soil"), &rl("clay")));
        assert!(!tags.contains(&rl("soil"), &rl("mud")), "replace drops the base entries");
        assert!(tags.contains(&rl("mineable/axe"), &rl("oak_log")));
        assert!(!tags.contains(&rl("nonexistent"), &rl("dirt")));

        fs::remove_dir_all(base).unwrap();
        fs::remove_dir_all(over).unwrap();
    }

    #[test]
    fn ordered_members_follow_the_tag_loader() {
        let root = temp("order");
        write(&root, "plants.json", r#"{"values":["minecraft:b","minecraft:a"]}"#);
        write(&root, "all.json", r##"{"values":["minecraft:c","#minecraft:plants","minecraft:b","minecraft:d"]}"##);
        let tags = load_tags(&[&root], "block").unwrap();
        let ids: Vec<&str> = tags.ordered(&rl("all")).unwrap().iter().map(|id| id.path()).collect();
        assert_eq!(ids, ["c", "b", "a", "d"], "nested tags expand in place, first appearance wins");
        assert!(tags.ordered(&rl("absent")).is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_cycle_is_an_error_and_an_optional_missing_tag_is_not() {
        let cyclic = temp("cycle");
        write(&cyclic, "a.json", r##"{"values":["#minecraft:b"]}"##);
        write(&cyclic, "b.json", r##"{"values":["#minecraft:a"]}"##);
        assert!(matches!(
            load_tags(&[&cyclic], "block"),
            Err(DatapackError::Cycle { .. })
        ));
        fs::remove_dir_all(cyclic).unwrap();

        let optional = temp("optional");
        write(
            &optional,
            "a.json",
            r##"{"values":[{"id":"#minecraft:absent","required":false},"minecraft:stone"]}"##,
        );
        let tags = load_tags(&[&optional], "block").unwrap();
        assert!(tags.contains(&rl("a"), &rl("stone")));
        fs::remove_dir_all(optional).unwrap();
    }

    /// The real 26.2 block tags, when the local reference tree has them (it is never committed).
    #[test]
    fn loads_the_reference_block_tags_when_present() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference");
        if !root.join("data/minecraft/tags/block").is_dir() {
            return;
        }
        let tags = load_tags(&[&root], "block").unwrap();
        assert!(tags.contains(&rl("dirt"), &rl("dirt")));
        assert!(tags.len() > 100);
    }
}
