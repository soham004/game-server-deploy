//! Commands learned from a running server's `help`: the commands mods and plugins add, which the
//! vanilla data generator doesn't know.
//!
//! Seen on real 26.3 servers:
//! - Vanilla, Fabric, Forge and `NeoForge` print one line per command, Brigadier's "smart usage",
//!   after `System chat: ` on 26.x and bare before: `/give <targets> <item> [<count>]`,
//!   `/chunky [cancel|center|…]`, `/clone (<begin>|from)`, `/xp -> experience`.
//! - Paper and Purpur print Bukkit's index, all on one page for the console, every command with what
//!   it does: `/chunky: Generates chunks`, `/bukkit:version: Gets the version of this server…`.
//!
//! A usage goes one level deep past alternatives, so a learned command completes its name and its
//! first words; what follows is free text.

use serde::{Deserialize, Serialize};

use super::tree::{CommandTree, Node, NodeId, TreeError, UNKNOWN_PARSER};

/// More commands than any server has; the rest of a longer answer is ignored.
pub const MAX_LEARNED: usize = 2_000;
/// Lines longer than this aren't a command's usage (`/gamerule`'s, the longest, is about 4,000).
const MAX_LINE: usize = 16_384;
/// Alternatives kept from one group.
const MAX_ALTERNATIVES: usize = 300;

/// One command as `help` describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnedCommand {
    pub name: String,
    /// What follows the name in a usage line (`<targets> <item> [<count>]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<String>,
    /// What it does, from Bukkit's index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// The commands in `help`'s output; lines that aren't one (headers, other output) are skipped.
pub fn parse_help<'a>(lines: impl IntoIterator<Item = &'a str>) -> Vec<LearnedCommand> {
    let mut commands: Vec<LearnedCommand> = Vec::new();
    for line in lines {
        if commands.len() >= MAX_LEARNED {
            break;
        }
        if let Some(command) = parse_line(line)
            && !commands.iter().any(|known| known.name == command.name)
        {
            commands.push(command);
        }
    }
    commands
}

/// True for a line `help` prints about one command (either form).
pub fn is_help_line(line: &str) -> bool {
    parse_line(line).is_some()
}

fn parse_line(line: &str) -> Option<LearnedCommand> {
    if line.len() > MAX_LINE {
        return None;
    }
    let line = line.trim_end();
    let line = line.strip_prefix("System chat: ").unwrap_or(line);
    let body = line.strip_prefix('/')?;
    // Bukkit: "/name: what it does". A usage never has ": " right after the name.
    if let Some((name, description)) = body.split_once(": ")
        && is_command_name(name)
    {
        return Some(LearnedCommand {
            name: name.to_owned(),
            usage: None,
            description: Some(description.trim().to_owned()).filter(|text| !text.is_empty()),
        });
    }
    let (name, usage) = body.split_once(' ').unwrap_or((body, ""));
    is_command_name(name).then(|| LearnedCommand {
        name: name.to_owned(),
        usage: Some(usage.trim().to_owned()).filter(|text| !text.is_empty()),
        description: None,
    })
}

fn is_command_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'))
        && !name.starts_with(':')
        && !name.ends_with(':')
}

impl CommandTree {
    /// Adds the learned commands this tree doesn't have; the tree's own (the data generator's,
    /// exact) win. Aliases (`/xp -> experience`, Paper's `chunky:chunky`) point at their command.
    /// Returns how many were added.
    pub fn learn(&mut self, commands: &[LearnedCommand]) -> Result<usize, TreeError> {
        let mut added = 0;
        let mut aliases: Vec<(NodeId, String)> = Vec::new();
        for command in commands.iter().take(MAX_LEARNED) {
            if self.child(Self::ROOT, &command.name).is_some() {
                continue;
            }
            let mut root = Node::literal(&command.name);
            root.description.clone_from(&command.description);
            let id = self.add(Self::ROOT, root)?;
            added += 1;
            let usage = command.usage.as_deref().unwrap_or("");
            if let Some(target) = usage.strip_prefix("->") {
                aliases.push((id, target.trim().to_owned()));
                continue;
            }
            if command.usage.is_none()
                && let Some((_, bare)) = command.name.split_once(':')
            {
                aliases.push((id, bare.to_owned()));
            }
            self.node_mut(id).usage = Some(format!("{} {usage}", command.name).trim_end().to_owned());
            self.add_usage(id, usage)?;
        }
        for (id, target) in aliases {
            match self.child(Self::ROOT, &target) {
                Some(target) if target != id => self.node_mut(id).redirect = Some(target),
                _ => self.node_mut(id).executable = true,
            }
        }
        Ok(added)
    }

    /// Builds the nodes a usage describes under `command`.
    fn add_usage(&mut self, command: NodeId, usage: &str) -> Result<(), TreeError> {
        let mut at = command;
        for element in split_top_level(usage) {
            if element == "..." {
                self.node_mut(at).redirect = Some(Self::ROOT);
                return Ok(());
            }
            let (optional, inner) = if let Some(inner) = wrapped(&element, '[', ']') {
                (true, inner)
            } else if let Some(inner) = wrapped(&element, '(', ')') {
                (false, inner)
            } else {
                (false, element.as_str())
            };
            if optional {
                self.node_mut(at).executable = true;
            }
            let alternatives = split_alternatives(inner);
            if alternatives.len() > 1 {
                // Brigadier's usage stops at alternatives: each may go on, but it doesn't say how.
                for alternative in alternatives.into_iter().take(MAX_ALTERNATIVES) {
                    let mut node = node_for(&alternative);
                    node.executable = true;
                    self.add(at, node)?;
                }
                return Ok(());
            }
            at = self.add(at, node_for(inner))?;
        }
        self.node_mut(at).executable = true;
        Ok(())
    }
}

/// `<name>` is an argument (of unknown type); anything else a literal (its first word).
fn node_for(text: &str) -> Node {
    let text = text.trim();
    match wrapped(text, '<', '>') {
        Some(name) => Node::argument(name, UNKNOWN_PARSER),
        None => Node::literal(text.split_whitespace().next().unwrap_or(text)),
    }
}

/// `text` without its enclosing `open`…`close`, if it is one bracketed group.
fn wrapped(text: &str, open: char, close: char) -> Option<&str> {
    let inner = text.strip_prefix(open)?.strip_suffix(close)?;
    (depth_after(inner) == 0).then_some(inner)
}

/// Splits at spaces outside brackets.
fn split_top_level(text: &str) -> Vec<String> {
    split_outside_brackets(text, ' ')
}

/// Splits a group's inside at `|` outside nested brackets.
fn split_alternatives(text: &str) -> Vec<String> {
    split_outside_brackets(text, '|')
}

fn split_outside_brackets(text: &str, separator: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0_i32;
    for c in text.chars() {
        match c {
            '(' | '[' | '<' => depth += 1,
            ')' | ']' | '>' => depth -= 1,
            _ => {}
        }
        if c == separator && depth == 0 {
            if !current.trim().is_empty() {
                parts.push(current.trim().to_owned());
            }
            current.clear();
        } else {
            current.push(c);
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_owned());
    }
    parts
}

/// The bracket depth at the end of `text`; not 0 if it opens or closes more than it should.
fn depth_after(text: &str) -> i32 {
    let mut depth = 0_i32;
    for c in text.chars() {
        match c {
            '(' | '[' | '<' => depth += 1,
            ')' | ']' | '>' => {
                depth -= 1;
                if depth < 0 {
                    return depth;
                }
            }
            _ => {}
        }
    }
    depth
}
