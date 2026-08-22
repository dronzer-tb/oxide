//! Resource locations (`namespace:path` identifiers) and block state descriptors.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

pub const DEFAULT_NAMESPACE: &str = "minecraft";

/// A `namespace:path` identifier, matching vanilla's `ResourceLocation`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResourceLocation {
    namespace: String,
    path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResourceLocationParseError {
    #[error("resource location path must not be empty")]
    EmptyPath,
    #[error("resource location namespace must not be empty")]
    EmptyNamespace,
}

impl ResourceLocation {
    pub fn new(namespace: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            path: path.into(),
        }
    }

    /// Builds a `ResourceLocation` defaulting the namespace to `minecraft`.
    pub fn minecraft(path: impl Into<String>) -> Self {
        Self {
            namespace: DEFAULT_NAMESPACE.to_string(),
            path: path.into(),
        }
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}

impl fmt::Display for ResourceLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.namespace, self.path)
    }
}

impl FromStr for ResourceLocation {
    type Err = ResourceLocationParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.split_once(':') {
            Some((ns, path)) => {
                if path.is_empty() {
                    return Err(ResourceLocationParseError::EmptyPath);
                }
                let ns = if ns.is_empty() { DEFAULT_NAMESPACE } else { ns };
                Ok(Self::new(ns, path))
            }
            None => {
                if s.is_empty() {
                    return Err(ResourceLocationParseError::EmptyPath);
                }
                Ok(Self::minecraft(s))
            }
        }
    }
}

/// A vanilla biome id is just a `ResourceLocation` (e.g. `minecraft:plains`).
pub type BiomeId = ResourceLocation;

/// A block state: the block's identifier plus its property assignments, sorted by key so
/// two equal states always compare/hash the same way regardless of insertion order — this
/// matters for `PalettedContainer` deduplication.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlockState {
    pub name: ResourceLocation,
    pub properties: BTreeMap<String, String>,
}

impl BlockState {
    pub fn new(name: ResourceLocation) -> Self {
        Self {
            name,
            properties: BTreeMap::new(),
        }
    }

    pub fn with_property(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.properties.insert(key.into(), value.into());
        self
    }
}

impl fmt::Display for BlockState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.properties.is_empty() {
            write!(f, "{}", self.name)
        } else {
            write!(f, "{}[", self.name)?;
            for (i, (k, v)) in self.properties.iter().enumerate() {
                if i > 0 {
                    write!(f, ",")?;
                }
                write!(f, "{k}={v}")?;
            }
            write!(f, "]")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_round_trip() {
        let rl = ResourceLocation::new("oxide", "test_block");
        assert_eq!(rl.to_string(), "oxide:test_block");
    }

    #[test]
    fn from_str_defaults_namespace() {
        let rl: ResourceLocation = "stone".parse().unwrap();
        assert_eq!(rl.namespace(), "minecraft");
        assert_eq!(rl.path(), "stone");
    }

    #[test]
    fn from_str_explicit_namespace() {
        let rl: ResourceLocation = "oxide:test_block".parse().unwrap();
        assert_eq!(rl.namespace(), "oxide");
        assert_eq!(rl.path(), "test_block");
    }

    #[test]
    fn from_str_rejects_empty_path() {
        assert!("oxide:".parse::<ResourceLocation>().is_err());
        assert!("".parse::<ResourceLocation>().is_err());
    }

    #[test]
    fn block_state_display() {
        let bs = BlockState::new(ResourceLocation::minecraft("furnace"))
            .with_property("facing", "north")
            .with_property("lit", "false");
        assert_eq!(bs.to_string(), "minecraft:furnace[facing=north,lit=false]");
    }
}
