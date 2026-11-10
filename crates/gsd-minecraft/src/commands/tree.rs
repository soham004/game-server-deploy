//! The command tree: Brigadier's, as the vanilla data generator dumps it (`reports/commands.json`),
//! plus the commands a modded server's `help` lists (see `help.rs`).
//!
//! Nodes live in one list and point at each other by index, so a redirect (`execute as <targets>`
//! goes back to `execute`, `execute run` to the root) is just another index, and the tree can be
//! saved and loaded as plain JSON.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Far more nodes than a real tree has (26.3: about 5,000); a report with more is refused.
pub const MAX_NODES: usize = 100_000;
/// Deeper than a real tree goes (26.3: 14 levels).
const MAX_DEPTH: usize = 64;

pub type NodeId = u32;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandTree {
    nodes: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// The literal's text or the argument's name; empty for the root.
    pub name: String,
    pub kind: NodeKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<NodeId>,
    /// The command can end here.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub executable: bool,
    /// Where the command continues after this node: with that node's children.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect: Option<NodeId>,
    /// What the command does (Paper's help says, e.g. "Generates chunks").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// How the server's `help` writes the command, for commands learned from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeKind {
    Root,
    Literal,
    Argument {
        /// Brigadier's parser id, e.g. `minecraft:entity`; [`UNKNOWN_PARSER`] for learned ones.
        parser: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        properties: BTreeMap<String, Value>,
    },
}

/// The parser of arguments learned from `help`, which doesn't say what they take.
pub const UNKNOWN_PARSER: &str = "gsd:unknown";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TreeError {
    #[error("not a command tree: {0}")]
    Invalid(String),
    #[error("the command tree has more than {MAX_NODES} nodes")]
    TooLarge,
    #[error("the command tree is nested more than {MAX_DEPTH} levels deep")]
    TooDeep,
}

impl Node {
    fn new(name: &str, kind: NodeKind) -> Self {
        Self {
            name: name.to_owned(),
            kind,
            children: Vec::new(),
            executable: false,
            redirect: None,
            description: None,
            usage: None,
        }
    }

    pub fn literal(name: &str) -> Self {
        Self::new(name, NodeKind::Literal)
    }

    pub fn argument(name: &str, parser: &str) -> Self {
        Self::new(
            name,
            NodeKind::Argument {
                parser: parser.to_owned(),
                properties: BTreeMap::new(),
            },
        )
    }

    pub fn is_literal(&self) -> bool {
        matches!(self.kind, NodeKind::Literal)
    }

    /// The parser id of an argument.
    pub fn parser(&self) -> Option<&str> {
        match &self.kind {
            NodeKind::Argument { parser, .. } => Some(parser),
            _ => None,
        }
    }

    pub fn property(&self, key: &str) -> Option<&Value> {
        match &self.kind {
            NodeKind::Argument { properties, .. } => properties.get(key),
            _ => None,
        }
    }

    /// As Brigadier writes it in a usage: `give`, `<targets>`.
    pub fn display(&self) -> String {
        if self.is_literal() {
            self.name.clone()
        } else {
            format!("<{}>", self.name)
        }
    }
}

impl Default for CommandTree {
    fn default() -> Self {
        Self::empty()
    }
}

impl CommandTree {
    pub const ROOT: NodeId = 0;

    /// Just the root: no commands.
    pub fn empty() -> Self {
        Self {
            nodes: vec![Node::new("", NodeKind::Root)],
        }
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize]
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.len() <= 1
    }

    /// The commands: the root's children.
    pub fn commands(&self) -> impl Iterator<Item = &Node> {
        self.node(Self::ROOT).children.iter().map(|&id| self.node(id))
    }

    /// A child by name (a literal's text or an argument's name).
    pub fn child(&self, parent: NodeId, name: &str) -> Option<NodeId> {
        self.node(parent)
            .children
            .iter()
            .copied()
            .find(|&id| self.node(id).name == name)
    }

    /// The node whose children follow `id`: its redirect, or itself.
    pub fn continuation(&self, id: NodeId) -> NodeId {
        self.node(id).redirect.unwrap_or(id)
    }

    /// Adds `node` under `parent`; fails past [`MAX_NODES`].
    pub fn add(&mut self, parent: NodeId, node: Node) -> Result<NodeId, TreeError> {
        if self.nodes.len() >= MAX_NODES {
            return Err(TreeError::TooLarge);
        }
        let id = NodeId::try_from(self.nodes.len()).map_err(|_| TreeError::TooLarge)?;
        self.nodes.push(node);
        self.nodes[parent as usize].children.push(id);
        Ok(id)
    }

    pub(crate) fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id as usize]
    }

    /// Reads the data generator's `commands.json`.
    ///
    /// A redirect is a path of names from the root (`["execute"]`). The generator leaves out a
    /// redirect to the root itself (`execute run`), so a node that neither ends a command nor has
    /// children nor redirects is read as going back to the root.
    pub fn from_report(report: &Value) -> Result<Self, TreeError> {
        let root = report
            .as_object()
            .filter(|root| root.get("type").and_then(Value::as_str) == Some("root"))
            .ok_or_else(|| TreeError::Invalid("the top level isn't the root".into()))?;
        let mut tree = Self::empty();
        let mut redirects: Vec<(NodeId, Vec<String>)> = Vec::new();
        // Children are added in order, parent first, without recursion.
        let mut pending: Vec<(NodeId, &Map<String, Value>, usize)> = vec![(Self::ROOT, root, 0)];
        while let Some((parent, object, depth)) = pending.pop() {
            if depth > MAX_DEPTH {
                return Err(TreeError::TooDeep);
            }
            let Some(children) = object.get("children").and_then(Value::as_object) else {
                continue;
            };
            for (name, child) in children {
                let child = child
                    .as_object()
                    .ok_or_else(|| TreeError::Invalid(format!("{name} isn't a node")))?;
                let id = tree.add(parent, read_node(name, child)?)?;
                if let Some(path) = child.get("redirect") {
                    redirects.push((id, read_path(name, path)?));
                }
                pending.push((id, child, depth + 1));
            }
        }
        for (id, path) in redirects {
            let target = tree.find_path(&path).ok_or_else(|| {
                TreeError::Invalid(format!("a redirect to an unknown node: {}", path.join(" ")))
            })?;
            tree.node_mut(id).redirect = Some(target);
        }
        for index in 1..tree.nodes.len() {
            let node = &mut tree.nodes[index];
            if !node.executable && node.children.is_empty() && node.redirect.is_none() {
                node.redirect = Some(Self::ROOT);
            }
        }
        Ok(tree)
    }

    /// The node at a path of names from the root.
    pub fn find_path(&self, path: &[String]) -> Option<NodeId> {
        path.iter().try_fold(Self::ROOT, |at, name| self.child(at, name))
    }
}

fn read_node(name: &str, object: &Map<String, Value>) -> Result<Node, TreeError> {
    let kind = match object.get("type").and_then(Value::as_str) {
        Some("literal") => NodeKind::Literal,
        Some("argument") => NodeKind::Argument {
            parser: object
                .get("parser")
                .and_then(Value::as_str)
                .ok_or_else(|| TreeError::Invalid(format!("the argument {name} has no parser")))?
                .to_owned(),
            properties: object
                .get("properties")
                .and_then(Value::as_object)
                .map(|properties| {
                    properties
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect()
                })
                .unwrap_or_default(),
        },
        other => {
            return Err(TreeError::Invalid(format!("{name} has the type {other:?}")));
        }
    };
    let mut node = Node::new(name, kind);
    node.executable = object.get("executable").and_then(Value::as_bool).unwrap_or(false);
    Ok(node)
}

fn read_path(name: &str, path: &Value) -> Result<Vec<String>, TreeError> {
    path.as_array()
        .and_then(|segments| {
            segments
                .iter()
                .map(|segment| segment.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
        })
        .ok_or_else(|| TreeError::Invalid(format!("{name} has a redirect that isn't a list of names")))
}
