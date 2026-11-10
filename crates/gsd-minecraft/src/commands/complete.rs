//! Completing a console command as it is typed: what can come next at the cursor, and the
//! command's shape so far (`give <targets> <item> [<count>]`, the argument being typed marked).
//!
//! A pure function of the tree, what's online and the ids the game knows. The text before the
//! cursor is walked from the root: literals must match exactly, arguments are read the way their
//! parser reads them closely enough to find where they end (three words for a position, the rest of
//! the line for a message, brackets and quotes kept together). Every way through is tried, so
//! `tp` finds both `<destination>` and `<location>`. Where the text runs out is what's being typed.

use std::collections::HashSet;

use serde::Serialize;
use serde_json::Value;

use super::{
    registries::Registries,
    tree::{CommandTree, Node, NodeId, UNKNOWN_PARSER},
};

/// Suggestions returned at most.
pub const MAX_SUGGESTIONS: usize = 50;
/// Partial readings explored at most (a long `execute` chain is a few hundred).
const MAX_STEPS: usize = 20_000;
/// Alternatives shown in the signature before "…".
const MAX_SHOWN_ALTERNATIVES: usize = 5;
/// Parts of the signature after the one being typed.
const MAX_FOLLOWING_PARTS: usize = 8;

/// What the completer knows besides the tree.
#[derive(Debug, Clone, Copy)]
pub struct Sources<'a> {
    /// Names of the players online now.
    pub players: &'a [String],
    pub registries: &'a Registries,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Completion {
    pub suggestions: Vec<Suggestion>,
    pub signature: Option<Signature>,
}

/// One thing that can be typed at the cursor; it replaces `from..to` (offsets in UTF-16 code
/// units, as a browser's text field counts them).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Suggestion {
    pub text: String,
    pub from: usize,
    pub to: usize,
    pub kind: SuggestionKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionKind {
    Command,
    Literal,
    Player,
    Selector,
    Coordinates,
    Value,
    Id,
    Tag,
}

/// The command's shape: what was typed, the part being typed (`current`), and what follows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Signature {
    pub parts: Vec<SignaturePart>,
    /// What the argument being typed takes, e.g. "a whole number, at least 1".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignaturePart {
    pub text: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub current: bool,
}

/// How far a node reads into the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reading {
    /// It ends `len` bytes in, before a space; the command goes on after that space.
    Complete(usize),
    /// The text ends inside it: this is what's being typed.
    Partial,
    No,
}

/// A place the walk reached: `parent`'s children come next, from byte `at`.
#[derive(Debug, Clone)]
struct State {
    parent: NodeId,
    at: usize,
    path: Vec<NodeId>,
}

pub fn complete(tree: &CommandTree, sources: Sources<'_>, input: &str, cursor_utf16: usize) -> Completion {
    let cursor = byte_index(input, cursor_utf16);
    let text = &input[..cursor];
    let start = usize::from(text.starts_with('/'));
    let frontiers = walk(tree, text, start);

    // The word being typed may go on past the cursor ("gi|ve"): a suggestion replaces all of it.
    let word_end = cursor + input[cursor..].find(' ').unwrap_or(input.len() - cursor);
    let to = utf16_len(&input[..word_end]);
    let mut seen = HashSet::new();
    let mut ranked: Vec<(u8, Suggestion)> = Vec::new();
    for state in &frontiers {
        let typed = &text[state.at..];
        let from = utf16_len(&text[..state.at]);
        for &child in &tree.node(state.parent).children {
            let node = tree.node(child);
            for (rank, text, kind, detail) in candidates(tree, state.parent, node, typed, sources) {
                // The same text from a deeper reading (`tp <targets> <location>` for `tp ~ ~`)
                // would only confuse: the shallowest one is kept.
                if seen.insert(text.clone()) {
                    ranked.push((
                        rank,
                        Suggestion {
                            text,
                            from,
                            to,
                            kind,
                            detail,
                        },
                    ));
                }
            }
        }
    }
    ranked.sort_by(|(rank_a, a), (rank_b, b)| {
        rank_a
            .cmp(rank_b)
            .then(a.kind.cmp(&b.kind))
            .then_with(|| a.text.cmp(&b.text))
    });
    let suggestions = ranked
        .into_iter()
        .take(MAX_SUGGESTIONS)
        .map(|(_, suggestion)| suggestion)
        .collect();
    let signature = frontiers
        .iter()
        .max_by_key(|state| state.path.len())
        .filter(|state| !state.path.is_empty())
        .map(|state| signature(tree, state, &text[state.at..]));
    Completion {
        suggestions,
        signature,
    }
}

/// Every place the text runs out at, along every reading of it.
fn walk(tree: &CommandTree, text: &str, start: usize) -> Vec<State> {
    let mut frontiers = Vec::new();
    let mut pending = vec![State {
        parent: CommandTree::ROOT,
        at: start,
        path: Vec::new(),
    }];
    let mut steps = 0;
    while let Some(state) = pending.pop() {
        steps += 1;
        if steps > MAX_STEPS {
            break;
        }
        let rest = &text[state.at..];
        let mut typing_here = false;
        for &child in &tree.node(state.parent).children {
            match read(tree.node(child), rest) {
                Reading::Partial => typing_here = true,
                Reading::Complete(len) => {
                    let mut path = state.path.clone();
                    path.push(child);
                    pending.push(State {
                        parent: tree.continuation(child),
                        at: state.at + len + 1,
                        path,
                    });
                }
                Reading::No => {}
            }
        }
        if typing_here {
            frontiers.push(state);
        }
    }
    // Shallow first: the walk order is a stack's.
    frontiers.sort_by_key(|state| state.at);
    frontiers
}

fn read(node: &Node, rest: &str) -> Reading {
    match node.parser() {
        None => {
            let (len, complete) = word(rest);
            if !complete {
                // Being typed: whether it matches is for the suggestions to say.
                return Reading::Partial;
            }
            if rest[..len] == node.name {
                Reading::Complete(len)
            } else {
                Reading::No
            }
        }
        Some(parser) => read_argument(parser, node, rest),
    }
}

fn read_argument(parser: &str, node: &Node, rest: &str) -> Reading {
    if let Some(count) = coordinate_words(parser) {
        return words(rest, count, is_coordinate);
    }
    let greedy = parser == "minecraft:message"
        || (parser == "brigadier:string" && node.property("type").and_then(Value::as_str) == Some("greedy"));
    if greedy {
        return Reading::Partial;
    }
    let check: fn(&str) -> bool = match parser {
        "brigadier:integer" | "brigadier:long" => is_integer,
        "brigadier:float" | "brigadier:double" => is_number,
        "brigadier:bool" => |word| word == "true" || word == "false",
        _ => |_| true,
    };
    words(rest, 1, check)
}

/// Positions take three words (`~ ~1 ~`), rotations and columns two.
fn coordinate_words(parser: &str) -> Option<usize> {
    match parser {
        "minecraft:block_pos" | "minecraft:vec3" => Some(3),
        "minecraft:vec2" | "minecraft:rotation" | "minecraft:column_pos" => Some(2),
        _ => None,
    }
}

/// Reads `count` words, each passing `check` once complete.
fn words(rest: &str, count: usize, check: fn(&str) -> bool) -> Reading {
    let mut at = 0;
    for index in 0..count {
        let (len, complete) = word(&rest[at..]);
        if !complete {
            return Reading::Partial;
        }
        if !check(&rest[at..at + len]) {
            return Reading::No;
        }
        at += len;
        if index + 1 < count {
            at += 1;
        }
    }
    Reading::Complete(at)
}

/// The length of the word at the start of `text`, and whether a space ends it (rather than the
/// text). Quotes and brackets keep their spaces: `@e[type=cow, limit=1]`, `{"text": "hi"}`.
fn word(text: &str) -> (usize, bool) {
    let mut depth = 0_u32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (index, c) in text.char_indices() {
        if let Some(open) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == open {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '[' | '{' | '(' => depth += 1,
            ']' | '}' | ')' => depth = depth.saturating_sub(1),
            ' ' if depth == 0 => return (index, true),
            _ => {}
        }
    }
    (text.len(), false)
}

fn is_integer(word: &str) -> bool {
    let digits = word.strip_prefix('-').unwrap_or(word);
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

fn is_number(word: &str) -> bool {
    let digits = word.strip_prefix('-').unwrap_or(word);
    let mut dots = 0;
    !digits.is_empty()
        && digits != "."
        && digits.chars().all(|c| {
            if c == '.' {
                dots += 1;
            }
            c.is_ascii_digit() || c == '.'
        })
        && dots <= 1
}

/// `10`, `-3.5`, `~`, `~-2`, `^1`.
fn is_coordinate(word: &str) -> bool {
    let number = word
        .strip_prefix('~')
        .or_else(|| word.strip_prefix('^'))
        .unwrap_or(word);
    (number.is_empty() && word != number) || is_number(number)
}

type Candidate = (u8, String, SuggestionKind, Option<String>);

/// What `node` offers for `typed`, each with its rank: 0 starts with what's typed, 1 contains it.
fn candidates(
    tree: &CommandTree,
    parent: NodeId,
    node: &Node,
    typed: &str,
    sources: Sources<'_>,
) -> Vec<Candidate> {
    if node.is_literal() {
        let kind = if parent == CommandTree::ROOT {
            SuggestionKind::Command
        } else {
            SuggestionKind::Literal
        };
        return match_rank(&node.name, typed)
            .map(|rank| vec![(rank, node.name.clone(), kind, literal_detail(tree, node))])
            .unwrap_or_default();
    }
    let mut found = Vec::new();
    for (text, kind) in values(node, typed, sources) {
        let rank = if kind == SuggestionKind::Id || kind == SuggestionKind::Tag {
            id_rank(&text, typed)
        } else {
            match_rank(&text, typed)
        };
        if let Some(rank) = rank {
            found.push((rank, display_id(&text, typed, kind), kind, None));
        }
    }
    found
}

fn literal_detail(tree: &CommandTree, node: &Node) -> Option<String> {
    if let Some(description) = &node.description {
        return Some(description.clone());
    }
    node.redirect
        .filter(|&target| target != CommandTree::ROOT)
        .map(|target| format!("same as {}", tree.node(target).name))
}

/// 0 when `text` starts with `typed` (any case), 1 when it contains it (two letters or more).
fn match_rank(text: &str, typed: &str) -> Option<u8> {
    let text = text.to_ascii_lowercase();
    let typed = typed.to_ascii_lowercase();
    if text.starts_with(&typed) {
        Some(0)
    } else if typed.len() >= 2 && text.contains(&typed) {
        Some(1)
    } else {
        None
    }
}

/// Ids match with or without their namespace: `dia` finds `minecraft:diamond`.
fn id_rank(id: &str, typed: &str) -> Option<u8> {
    let bare_typed = typed.strip_prefix('#').unwrap_or(typed);
    if bare_typed.contains(':') {
        return match_rank(id.strip_prefix('#').unwrap_or(id), bare_typed);
    }
    let path = id.strip_prefix('#').unwrap_or(id);
    let path = path.split_once(':').map_or(path, |(_, path)| path);
    match_rank(path, bare_typed)
}

/// Vanilla ids are typed without `minecraft:` unless the text so far has a namespace.
fn display_id(text: &str, typed: &str, kind: SuggestionKind) -> String {
    if !matches!(kind, SuggestionKind::Id | SuggestionKind::Tag) || typed.contains(':') {
        return text.to_owned();
    }
    let (hash, id) = text.strip_prefix('#').map_or(("", text), |id| ("#", id));
    match id.strip_prefix("minecraft:") {
        Some(path) => format!("{hash}{path}"),
        None => text.to_owned(),
    }
}

/// The values an argument can take that are worth listing.
fn values(node: &Node, typed: &str, sources: Sources<'_>) -> Vec<(String, SuggestionKind)> {
    let parser = node.parser().unwrap_or(UNKNOWN_PARSER);
    let registry = node.property("registry").and_then(Value::as_str);
    let ids = |registry: &str| -> Vec<(String, SuggestionKind)> {
        sources
            .registries
            .ids(registry)
            .iter()
            .map(|id| (id.clone(), SuggestionKind::Id))
            .collect()
    };
    let tags = |registry: &str| -> Vec<(String, SuggestionKind)> {
        sources
            .registries
            .tags(registry)
            .iter()
            .map(|id| (id.clone(), SuggestionKind::Tag))
            .collect()
    };
    if let Some(words) = fixed_words(parser) {
        return words
            .iter()
            .map(|word| ((*word).to_owned(), SuggestionKind::Value))
            .collect();
    }
    match parser {
        "minecraft:entity" | "minecraft:game_profile" | "minecraft:score_holder" => {
            people(node, parser, typed, sources)
        }
        "minecraft:item_stack" => ids("minecraft:item"),
        "minecraft:item_predicate" => [ids("minecraft:item"), tags("minecraft:item")].concat(),
        "minecraft:block_state" => ids("minecraft:block"),
        "minecraft:block_predicate" => [ids("minecraft:block"), tags("minecraft:block")].concat(),
        "minecraft:particle" => ids("minecraft:particle_type"),
        "minecraft:loot_table" => ids("minecraft:loot_table"),
        "minecraft:resource" | "minecraft:resource_key" | "minecraft:resource_selector" => {
            registry.map(ids).unwrap_or_default()
        }
        "minecraft:resource_or_tag" | "minecraft:resource_or_tag_key" => registry
            .map(|registry| [ids(registry), tags(registry)].concat())
            .unwrap_or_default(),
        "minecraft:resource_location" => match node.name.as_str() {
            "sound" => ids("minecraft:sound_event"),
            "recipe" => ids("minecraft:recipe"),
            "advancement" => ids("minecraft:advancement"),
            "loot_table" => ids("minecraft:loot_table"),
            _ => Vec::new(),
        },
        "minecraft:dimension" => ["minecraft:overworld", "minecraft:the_nether", "minecraft:the_end"]
            .iter()
            .map(|id| ((*id).to_owned(), SuggestionKind::Id))
            .collect(),
        _ => coordinate_words(parser)
            .map(|count| {
                let relative = vec!["~"; count].join(" ");
                let local = vec!["^"; count].join(" ");
                let mut both = vec![(relative, SuggestionKind::Coordinates)];
                if count == 3 {
                    both.push((local, SuggestionKind::Coordinates));
                }
                both
            })
            .unwrap_or_default(),
    }
}

/// Arguments that take one of a few words.
fn fixed_words(parser: &str) -> Option<&'static [&'static str]> {
    let words: &'static [&'static str] = match parser {
        "minecraft:gamemode" => &["adventure", "creative", "spectator", "survival"],
        "brigadier:bool" => &["false", "true"],
        "minecraft:entity_anchor" => &["eyes", "feet"],
        "minecraft:heightmap" => &[
            "motion_blocking",
            "motion_blocking_no_leaves",
            "ocean_floor",
            "world_surface",
        ],
        "minecraft:template_rotation" => &["180", "clockwise_90", "counterclockwise_90", "none"],
        "minecraft:template_mirror" => &["front_back", "left_right", "none"],
        "minecraft:operation" => &["%=", "*=", "+=", "-=", "/=", "<", "=", ">", "><"],
        "minecraft:swizzle" => &["x", "xy", "xyz", "xz", "y", "yz", "z"],
        "minecraft:team_color" | "minecraft:color" => &COLORS,
        "minecraft:scoreboard_slot" => &["below_name", "list", "sidebar"],
        "minecraft:objective_criteria" => &[
            "air",
            "armor",
            "deathCount",
            "dummy",
            "food",
            "health",
            "level",
            "playerKillCount",
            "totalKillCount",
            "trigger",
            "xp",
        ],
        "minecraft:item_slot" => &[
            "armor.body",
            "armor.chest",
            "armor.feet",
            "armor.head",
            "armor.legs",
            "container.0",
            "hotbar.0",
            "weapon",
            "weapon.mainhand",
            "weapon.offhand",
        ],
        _ => return None,
    };
    Some(words)
}

const COLORS: [&str; 17] = [
    "aqua",
    "black",
    "blue",
    "dark_aqua",
    "dark_blue",
    "dark_gray",
    "dark_green",
    "dark_purple",
    "dark_red",
    "gold",
    "gray",
    "green",
    "light_purple",
    "red",
    "reset",
    "white",
    "yellow",
];

/// Players online, and the selectors the argument accepts.
fn people(node: &Node, parser: &str, typed: &str, sources: Sources<'_>) -> Vec<(String, SuggestionKind)> {
    if typed.contains('[') {
        return Vec::new();
    }
    let players_only = parser == "minecraft:game_profile"
        || node.property("type").and_then(Value::as_str) == Some("players");
    let single = node.property("amount").and_then(Value::as_str) == Some("single");
    let mut selectors: Vec<&str> = if players_only {
        vec!["@a", "@p", "@r", "@s"]
    } else {
        vec!["@a", "@e", "@n", "@p", "@r", "@s"]
    };
    if single {
        selectors.retain(|selector| !matches!(*selector, "@a" | "@e"));
    }
    let mut found: Vec<(String, SuggestionKind)> = sources
        .players
        .iter()
        .map(|name| (name.clone(), SuggestionKind::Player))
        .collect();
    if parser == "minecraft:score_holder" {
        found.push(("*".to_owned(), SuggestionKind::Selector));
    }
    found.extend(
        selectors
            .into_iter()
            .map(|selector| (selector.to_owned(), SuggestionKind::Selector)),
    );
    found
}

/// The command's shape at `state`: what was walked, the next part (current), and what follows
/// when it is one way on.
fn signature(tree: &CommandTree, state: &State, typed: &str) -> Signature {
    let mut parts: Vec<SignaturePart> = state
        .path
        .iter()
        .map(|&id| SignaturePart {
            text: tree.node(id).display(),
            current: false,
        })
        .collect();
    let parent = tree.node(state.parent);
    let children: Vec<&Node> = parent.children.iter().map(|&id| tree.node(id)).collect();
    let slot = group(&children, parent.executable);
    parts.push(SignaturePart {
        text: slot,
        current: true,
    });
    if let [only] = parent.children.as_slice() {
        parts.extend(following(tree, *only));
    }
    // The argument the typed text is for: the only one, or the first that reads it.
    let argument = children
        .iter()
        .filter(|node| !node.is_literal())
        .find(|node| read(node, typed) != Reading::No)
        .or_else(|| children.iter().find(|node| !node.is_literal()));
    Signature {
        parts,
        hint: argument.and_then(|node| hint(node)),
    }
}

/// `<item>`, `(add|remove|<name>)`, or `[…]` when the command may end before it.
fn group(children: &[&Node], optional: bool) -> String {
    let mut names: Vec<String> = children
        .iter()
        .take(MAX_SHOWN_ALTERNATIVES)
        .map(|node| node.display())
        .collect();
    if children.len() > MAX_SHOWN_ALTERNATIVES {
        names.push("…".to_owned());
    }
    let inside = names.join("|");
    match (optional, children.len()) {
        (true, _) => format!("[{inside}]"),
        (false, 0 | 1) => inside,
        (false, _) => format!("({inside})"),
    }
}

/// What follows `from` while there is one way on, as Brigadier's usage writes it.
fn following(tree: &CommandTree, from: NodeId) -> Vec<SignaturePart> {
    let mut parts = Vec::new();
    let mut at = tree.node(from);
    while parts.len() < MAX_FOLLOWING_PARTS {
        if at.redirect == Some(CommandTree::ROOT) {
            parts.push("…".to_owned());
            break;
        }
        if at.redirect.is_some() || at.children.is_empty() {
            break;
        }
        let children: Vec<&Node> = at.children.iter().map(|&id| tree.node(id)).collect();
        parts.push(group(&children, at.executable));
        match at.children.as_slice() {
            [only] => at = tree.node(*only),
            _ => break,
        }
    }
    parts
        .into_iter()
        .map(|text| SignaturePart { text, current: false })
        .collect()
}

/// What an argument takes, in words.
fn hint(node: &Node) -> Option<String> {
    let parser = node.parser()?;
    let range = || {
        let min = node.property("min").map(Value::to_string);
        let max = node.property("max").map(Value::to_string);
        match (min, max) {
            (Some(min), Some(max)) => format!(", {min} to {max}"),
            (Some(min), None) => format!(", at least {min}"),
            (None, Some(max)) => format!(", at most {max}"),
            (None, None) => String::new(),
        }
    };
    let text = match parser {
        "brigadier:integer" | "brigadier:long" => format!("a whole number{}", range()),
        "brigadier:float" | "brigadier:double" => format!("a number{}", range()),
        "brigadier:bool" => "true or false".to_owned(),
        "brigadier:string" => match node.property("type").and_then(Value::as_str) {
            Some("greedy") => "text, to the end of the line".to_owned(),
            Some("phrase") => "a word, or text in quotes".to_owned(),
            _ => "a word".to_owned(),
        },
        "minecraft:message" => "a message, to the end of the line".to_owned(),
        "minecraft:entity" => {
            let what = if node.property("type").and_then(Value::as_str) == Some("players") {
                "player"
            } else {
                "player or entity"
            };
            let how_many = if node.property("amount").and_then(Value::as_str) == Some("single") {
                "one "
            } else {
                ""
            };
            format!("{how_many}{what}: a name or a selector (@p, @a, …)")
        }
        "minecraft:game_profile" => "a player: a name or a selector".to_owned(),
        "minecraft:score_holder" => "a name, a selector or *".to_owned(),
        "minecraft:block_pos" => "a block position: x y z (~ for relative, ^ for local)".to_owned(),
        "minecraft:vec3" => "a position: x y z (~ for relative, ^ for local)".to_owned(),
        "minecraft:vec2" | "minecraft:column_pos" => "x z (~ for relative)".to_owned(),
        "minecraft:rotation" => "rotation: yaw pitch (~ for relative)".to_owned(),
        "minecraft:item_stack" => "an item, e.g. diamond_sword".to_owned(),
        "minecraft:item_predicate" => "an item or #tag".to_owned(),
        "minecraft:block_state" => "a block, e.g. stone or oak_stairs[facing=east]".to_owned(),
        "minecraft:block_predicate" => "a block or #tag".to_owned(),
        "minecraft:component" => "text as JSON or SNBT, e.g. {\"text\":\"Hi\"}".to_owned(),
        "minecraft:nbt_compound_tag" | "minecraft:nbt_tag" => "NBT, e.g. {Count:1b}".to_owned(),
        "minecraft:nbt_path" => "an NBT path, e.g. Inventory[0].id".to_owned(),
        "minecraft:time" => "a time: ticks, or with d, s or t (1d, 20s)".to_owned(),
        "minecraft:int_range" | "minecraft:float_range" => "a number or range, e.g. 1..5".to_owned(),
        "minecraft:uuid" => "a UUID".to_owned(),
        "minecraft:function" => "a function, e.g. namespace:name".to_owned(),
        UNKNOWN_PARSER => "what the command's mod or plugin takes here".to_owned(),
        "minecraft:resource"
        | "minecraft:resource_key"
        | "minecraft:resource_or_tag"
        | "minecraft:resource_or_tag_key" => {
            let registry = node
                .property("registry")
                .and_then(Value::as_str)
                .unwrap_or("resource");
            let name = registry.rsplit('/').next().unwrap_or(registry);
            let name = name.trim_start_matches("minecraft:").replace('_', " ");
            format!("a {name} id")
        }
        _ => return None,
    };
    Some(text)
}

/// The byte index of a UTF-16 offset (clamped to the text, never inside a character).
pub fn byte_index(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (index, c) in text.char_indices() {
        if units >= utf16 {
            return index;
        }
        units += c.len_utf16();
    }
    text.len()
}

pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}
