//! The shared block vocabulary: what a block *is*, independent of how it is stored
//! and how it is sent.
//!
//! This crate exists because `bedrock-world` and `bedrock-protocol` both need to name
//! blocks and neither may depend on the other — see ADR-008 in `docs/DECISIONS.md`.
//! It is a leaf crate: no internal dependencies, no I/O.
//!
//! **What lives here:** the identity of a block — its namespaced name and its state
//! properties.
//!
//! **What does not:** the runtime-id palette. A runtime id is a per-version network
//! identifier, so the name-to-runtime-id mapping belongs to `bedrock-protocol`. Storage
//! uses names and properties, never runtime ids.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::borrow::Cow;
use std::fmt;

/// The value of one block state property.
///
/// Three shapes, because that is what Bedrock's block states carry: a flag, a number,
/// or a name from a fixed set.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Value {
    /// `true`/`false`, like `upside_down_bit`.
    Bool(bool),
    /// An integer, like `height`.
    Int(i32),
    /// A name from a fixed set, like `"north"`.
    Text(Cow<'static, str>),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(v) => write!(f, "{v}"),
            Self::Int(v) => write!(f, "{v}"),
            Self::Text(v) => write!(f, "{v}"),
        }
    }
}

/// A block state: a namespaced name plus the properties that distinguish it from its
/// siblings.
///
/// Two blocks are the same block when their names and their properties match, which is
/// what makes this the key of a chunk palette. Properties are kept **sorted by name**
/// so that equality does not depend on the order they were declared in — otherwise the
/// same stair, described twice, would take two palette slots.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Block {
    name: Cow<'static, str>,
    properties: Vec<(Cow<'static, str>, Value)>,
}

impl Block {
    /// A block with no state properties.
    pub const fn new(name: &'static str) -> Self {
        Self {
            name: Cow::Borrowed(name),
            properties: Vec::new(),
        }
    }

    /// A block with state properties, in any order.
    pub fn with_properties<N, I>(name: N, properties: I) -> Self
    where
        N: Into<Cow<'static, str>>,
        I: IntoIterator<Item = (Cow<'static, str>, Value)>,
    {
        let mut properties: Vec<_> = properties.into_iter().collect();
        properties.sort_by(|a, b| a.0.cmp(&b.0));
        properties.dedup_by(|a, b| a.0 == b.0);
        Self {
            name: name.into(),
            properties,
        }
    }

    /// The namespaced name, such as `minecraft:stone`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The state properties, sorted by name.
    pub fn properties(&self) -> &[(Cow<'static, str>, Value)] {
        &self.properties
    }

    /// The value of one property, if the block has it.
    pub fn property(&self, name: &str) -> Option<&Value> {
        self.properties
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    }

    /// Whether this is the block that means "nothing here".
    ///
    /// Air is asked about often enough — deciding how tall a column is, whether a
    /// section is worth sending — that spelling the comparison out at every call site
    /// is how one of them ends up spelling it differently.
    pub fn is_air(&self) -> bool {
        self.name == AIR.name
    }
}

impl fmt::Display for Block {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)?;
        if self.properties.is_empty() {
            return Ok(());
        }
        let pairs: Vec<String> = self
            .properties
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        write!(f, "[{}]", pairs.join(","))
    }
}

/// `minecraft:air`.
pub const AIR: Block = Block::new("minecraft:air");

/// `minecraft:stone`.
pub const STONE: Block = Block::new("minecraft:stone");

// Two blocks is the whole vocabulary the flat world needs. Names are cheap to add and
// nothing here validates them, but a constant for a block nothing places is a constant
// nothing checks — the palette in `bedrock-protocol` is where a name has to be real.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_is_its_name() {
        assert_eq!(STONE.name(), "minecraft:stone");
        assert!(STONE.properties().is_empty());
        assert_ne!(STONE, AIR);
    }

    /// The palette keys on this type, so two descriptions of the same state that differ
    /// only in declaration order must be one key, not two.
    #[test]
    fn property_order_does_not_change_identity() {
        let one = Block::with_properties(
            "minecraft:stone_stairs",
            [
                ("upside_down_bit".into(), Value::Bool(false)),
                ("weirdo_direction".into(), Value::Int(2)),
            ],
        );
        let other = Block::with_properties(
            "minecraft:stone_stairs",
            [
                ("weirdo_direction".into(), Value::Int(2)),
                ("upside_down_bit".into(), Value::Bool(false)),
            ],
        );
        assert_eq!(one, other);
        assert_eq!(one.to_string(), other.to_string());
    }

    #[test]
    fn a_property_can_be_read_back() {
        let block = Block::with_properties(
            "minecraft:torch",
            [("torch_facing_direction".into(), Value::Text("east".into()))],
        );
        assert_eq!(
            block.property("torch_facing_direction"),
            Some(&Value::Text("east".into()))
        );
        assert_eq!(block.property("nope"), None);
    }

    #[test]
    fn only_air_is_air() {
        assert!(AIR.is_air());
        assert!(!STONE.is_air());
    }

    /// Printed form is what a log or an error message shows, so it has to name the
    /// state and not just the block.
    #[test]
    fn a_block_prints_as_its_state() {
        assert_eq!(STONE.to_string(), "minecraft:stone");
        let stair = Block::with_properties(
            "minecraft:stone_stairs",
            [("upside_down_bit".into(), Value::Bool(true))],
        );
        assert_eq!(
            stair.to_string(),
            "minecraft:stone_stairs[upside_down_bit=true]"
        );
    }
}
