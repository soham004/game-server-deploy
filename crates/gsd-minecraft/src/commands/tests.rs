#![allow(clippy::unwrap_used)]

use std::{
    collections::BTreeSet,
    io::{Read, Write},
    path::PathBuf,
    sync::LazyLock,
};

use serde_json::{Value, json};

use super::{
    complete::{Completion, Sources, SuggestionKind, byte_index, complete, utf16_len},
    help::{self, LearnedCommand},
    registries::Registries,
    tree::{CommandTree, TreeError},
};

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn gzipped_json(name: &str) -> Value {
    let file = std::fs::File::open(fixture_path(name)).unwrap();
    let mut text = String::new();
    flate2::read::GzDecoder::new(file)
        .read_to_string(&mut text)
        .unwrap();
    serde_json::from_str(&text).unwrap()
}

fn help_fixture(name: &str) -> String {
    std::fs::read_to_string(fixture_path(name)).unwrap()
}

/// Real 26.3 data: the generator's tree and registries, and a few data-driven ids as the jar has
/// them.
static TREE: LazyLock<CommandTree> =
    LazyLock::new(|| CommandTree::from_report(&gzipped_json("commands_26.3.json.gz")).unwrap());
static REGISTRIES: LazyLock<Registries> = LazyLock::new(|| {
    let wanted = Registries::wanted(&TREE);
    let mut registries = Registries::default();
    registries.add_report(&gzipped_json("registries_26.3.json.gz"), &wanted);
    registries.add_data_files(
        [
            "data/minecraft/enchantment/sharpness.json",
            "data/minecraft/enchantment/smite.json",
            "data/minecraft/tags/item/logs.json",
            "data/minecraft/worldgen/biome/plains.json",
            "data/minecraft/loot_table/blocks/stone.json",
        ]
        .into_iter(),
        &wanted,
    );
    registries
});
const PLAYERS: [&str; 2] = ["Steve", "alex_2"];

fn run(input: &str) -> Completion {
    run_at(input, utf16_len(input))
}

fn run_at(input: &str, cursor: usize) -> Completion {
    run_with(&TREE, input, cursor)
}

fn run_with(tree: &CommandTree, input: &str, cursor: usize) -> Completion {
    let players: Vec<String> = PLAYERS.iter().map(|name| (*name).to_owned()).collect();
    complete(
        tree,
        Sources {
            players: &players,
            registries: &REGISTRIES,
        },
        input,
        cursor,
    )
}

fn texts(completion: &Completion) -> Vec<&str> {
    completion.suggestions.iter().map(|s| s.text.as_str()).collect()
}

fn signature(completion: &Completion) -> String {
    let signature = completion.signature.as_ref().unwrap();
    signature
        .parts
        .iter()
        .map(|part| {
            if part.current {
                format!("*{}*", part.text)
            } else {
                part.text.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ---- the tree ----

#[test]
fn the_real_tree_reads_with_its_redirects() {
    let tree = &*TREE;
    assert_eq!(tree.commands().count(), 94);
    let give = tree.child(CommandTree::ROOT, "give").unwrap();
    let targets = tree.child(give, "targets").unwrap();
    let item = tree.child(targets, "item").unwrap();
    let count = tree.child(item, "count").unwrap();
    assert_eq!(tree.node(targets).parser(), Some("minecraft:entity"));
    assert!(tree.node(item).executable && tree.node(count).executable);
    assert_eq!(tree.node(count).property("min"), Some(&json!(1)));

    let experience = tree.child(CommandTree::ROOT, "experience").unwrap();
    let xp = tree.child(CommandTree::ROOT, "xp").unwrap();
    assert_eq!(tree.node(xp).redirect, Some(experience));
    let execute = tree.child(CommandTree::ROOT, "execute").unwrap();
    // `execute run` goes back to the root: the generator leaves that redirect out.
    let run = tree.child(execute, "run").unwrap();
    assert_eq!(tree.node(run).redirect, Some(CommandTree::ROOT));
    let as_targets = tree
        .find_path(&["execute".into(), "as".into(), "targets".into()])
        .unwrap();
    assert_eq!(tree.node(as_targets).redirect, Some(execute));
}

#[test]
fn a_tree_survives_saving_and_loading() {
    let saved = serde_json::to_string(&*TREE).unwrap();
    let loaded: CommandTree = serde_json::from_str(&saved).unwrap();
    assert_eq!(&loaded, &*TREE);
}

#[test]
fn broken_reports_are_refused() {
    assert!(matches!(
        CommandTree::from_report(&json!({"type": "literal"})),
        Err(TreeError::Invalid(_))
    ));
    let bad_redirect =
        json!({"type": "root", "children": {"a": {"type": "literal", "redirect": ["nowhere"]}}});
    assert!(matches!(
        CommandTree::from_report(&bad_redirect),
        Err(TreeError::Invalid(_))
    ));
    let no_parser = json!({"type": "root", "children": {"a": {"type": "argument"}}});
    assert!(matches!(
        CommandTree::from_report(&no_parser),
        Err(TreeError::Invalid(_))
    ));

    let mut deep = json!({"type": "literal", "executable": true});
    for _ in 0..70 {
        deep = json!({"type": "literal", "children": {"a": deep}});
    }
    let deep = json!({"type": "root", "children": {"a": deep}});
    assert_eq!(CommandTree::from_report(&deep), Err(TreeError::TooDeep));
}

// ---- help ----

#[test]
fn vanilla_help_gives_every_command_with_its_usage() {
    let text = help_fixture("help_26.3_vanilla.txt");
    let commands = help::parse_help(text.lines());
    assert_eq!(
        commands.len(),
        92,
        "{:?}",
        commands.iter().map(|c| &c.name).collect::<Vec<_>>()
    );
    let give = commands.iter().find(|c| c.name == "give").unwrap();
    assert_eq!(give.usage.as_deref(), Some("<targets> <item> [<count>]"));
    let xp = commands.iter().find(|c| c.name == "xp").unwrap();
    assert_eq!(xp.usage.as_deref(), Some("-> experience"));

    // Learned alone (no generator), the usage becomes nodes.
    let mut tree = CommandTree::empty();
    assert_eq!(tree.learn(&commands).unwrap(), 92);
    let give = tree.child(CommandTree::ROOT, "give").unwrap();
    let targets = tree.child(give, "targets").unwrap();
    let item = tree.child(targets, "item").unwrap();
    let count = tree.child(item, "count").unwrap();
    assert!(!tree.node(give).executable && !tree.node(targets).executable);
    assert!(tree.node(item).executable, "[<count>] is optional");
    assert!(tree.node(count).executable);
    let experience = tree.child(CommandTree::ROOT, "experience").unwrap();
    assert_eq!(
        tree.node(tree.child(CommandTree::ROOT, "xp").unwrap()).redirect,
        Some(experience)
    );
    let difficulty = tree.child(CommandTree::ROOT, "difficulty").unwrap();
    assert!(tree.node(difficulty).executable);
    assert_eq!(tree.node(difficulty).children.len(), 4);
}

#[test]
fn a_modded_servers_help_adds_only_its_own_commands() {
    let text = help_fixture("help_26.3_neoforge.txt");
    let mut tree = TREE.clone();
    assert_eq!(tree.learn(&help::parse_help(text.lines())).unwrap(), 2);
    let chunky = tree.child(CommandTree::ROOT, "chunky").unwrap();
    assert!(tree.node(chunky).executable);
    assert!(tree.child(chunky, "start").is_some() && tree.child(chunky, "worldborder").is_some());
    assert!(tree.child(CommandTree::ROOT, "neoforge").is_some());

    let completion = run_with(&tree, "chunky st", 9);
    assert_eq!(texts(&completion), ["start"]);
}

#[test]
fn paper_help_gives_plugin_commands_with_what_they_do() {
    let text = help_fixture("help_26.3_paper.txt");
    let commands = help::parse_help(text.lines());
    let chunky = commands.iter().find(|c| c.name == "chunky").unwrap();
    assert_eq!(chunky.description.as_deref(), Some("Generates chunks"));
    assert!(commands.iter().all(|c| !c.name.contains(' ')));

    let mut tree = TREE.clone();
    let added = tree.learn(&commands).unwrap();
    assert!(added > 20, "{added}");
    // Namespaced names lead to the command they name.
    let give = tree.child(CommandTree::ROOT, "give").unwrap();
    let namespaced = tree.child(CommandTree::ROOT, "minecraft:give").unwrap();
    assert_eq!(tree.node(namespaced).redirect, Some(give));
    let chunky = tree.child(CommandTree::ROOT, "chunky").unwrap();
    assert_eq!(
        tree.node(tree.child(CommandTree::ROOT, "chunky:chunky").unwrap())
            .redirect,
        Some(chunky)
    );
    assert!(tree.child(CommandTree::ROOT, "version").is_some());

    let completion = run_with(&tree, "chun", 4);
    let found = completion
        .suggestions
        .iter()
        .find(|s| s.text == "chunky")
        .unwrap();
    assert_eq!(found.detail.as_deref(), Some("Generates chunks"));
    let completion = run_with(&tree, "minecraft:give @p dia", 21);
    assert!(
        texts(&completion).contains(&"diamond"),
        "{:?}",
        texts(&completion)
    );
}

#[test]
fn lines_that_arent_commands_are_skipped() {
    let lines = [
        "--------- Help: Index ---------------------------------",
        "Use /help [n] to get page n of help.",
        "Aliases: Lists command aliases",
        "/",
        "/ spaced",
        "/:bad: x",
        "/ok",
        "System chat: /also_ok <x>",
    ];
    let names: Vec<String> = help::parse_help(lines).into_iter().map(|c| c.name).collect();
    assert_eq!(names, ["ok", "also_ok"]);
    let long = format!("/long {}", "a".repeat(20_000));
    assert_eq!(help::parse_help([long.as_str()]), []);
}

#[test]
fn learned_commands_never_replace_the_generators() {
    let mut tree = TREE.clone();
    let before = tree.len();
    let fake = [LearnedCommand {
        name: "give".into(),
        usage: Some("<nonsense>".into()),
        description: None,
    }];
    assert_eq!(tree.learn(&fake).unwrap(), 0);
    assert_eq!(tree.len(), before);
}

// ---- completing ----

#[test]
fn commands_complete_from_nothing_and_from_their_start() {
    let all = run("");
    assert_eq!(all.suggestions.len(), super::complete::MAX_SUGGESTIONS);
    assert!(all.suggestions.iter().all(|s| s.kind == SuggestionKind::Command));
    assert!(all.signature.is_none());

    let gi = run("gi");
    assert_eq!(texts(&gi)[0], "give");
    assert_eq!((gi.suggestions[0].from, gi.suggestions[0].to), (0, 2));
    let slash = run("/gi");
    assert_eq!((slash.suggestions[0].from, slash.suggestions[0].to), (1, 3));
    // A word that only contains what's typed comes after those that start with it.
    let rule = run("rule");
    assert_eq!(texts(&rule), ["gamerule"]);
}

#[test]
fn give_offers_players_then_items_then_shows_the_count() {
    let targets = run("give ");
    assert_eq!(texts(&targets), ["Steve", "alex_2", "@a", "@p", "@r", "@s"]);
    assert_eq!(signature(&targets), "give *<targets>* <item> [<count>]");

    let items = run("give @p dia");
    assert!(
        texts(&items).starts_with(&["diamond", "diamond_axe"]),
        "{:?}",
        texts(&items)
    );
    assert_eq!(items.suggestions[0].from, 8);
    assert_eq!(signature(&items), "give <targets> *<item>* [<count>]");
    let namespaced = run("give @p minecraft:diamond_b");
    assert_eq!(
        texts(&namespaced),
        ["minecraft:diamond_block", "minecraft:diamond_boots"]
    );

    let count = run("give @p diamond ");
    assert_eq!(signature(&count), "give <targets> <item> *[<count>]*");
    assert_eq!(
        count.signature.unwrap().hint.as_deref(),
        Some("a whole number, at least 1")
    );
}

#[test]
fn every_reading_is_tried() {
    // `tp <destination>` and `tp <location>` at once.
    let tp = run("tp ");
    let found = texts(&tp);
    assert!(
        found.contains(&"Steve")
            && found.contains(&"@e")
            && found.contains(&"~ ~ ~")
            && found.contains(&"^ ^ ^"),
        "{found:?}"
    );
    assert_eq!(texts(&run("tp ~ ~")), ["~ ~ ~"]);
    // Typed numbers aren't second-guessed.
    assert_eq!(texts(&run("tp 10 64 ")), Vec::<&str>::new());
}

#[test]
fn execute_runs_into_any_command() {
    let completion = run("execute as @a at @s run gi");
    assert_eq!(texts(&completion), ["give"]);
    let selector = run("execute as @e[type=cow, limit=1] run say hi there");
    assert_eq!(selector.suggestions, []);
    assert_eq!(
        selector.signature.unwrap().hint.as_deref(),
        Some("a message, to the end of the line")
    );
}

#[test]
fn ids_come_from_the_registries_the_argument_names() {
    assert_eq!(texts(&run("effect give @p spe")), ["speed"]);
    assert_eq!(texts(&run("enchant @p sha")), ["sharpness"]);
    assert!(texts(&run("clear @p #lo")).contains(&"#logs"));
    assert!(texts(&run("gamerule keep")).contains(&"keep_inventory"));
    assert_eq!(texts(&run("gamemode cr")), ["creative"]);
    assert_eq!(texts(&run("difficulty pe")), ["peaceful"]);
}

#[test]
fn the_word_under_the_cursor_is_replaced_whole() {
    let completion = run_at("gi give", 2);
    assert_eq!(texts(&completion)[0], "give");
    assert_eq!(
        (completion.suggestions[0].from, completion.suggestions[0].to),
        (0, 2)
    );
    let middle = run_at("give", 2);
    assert_eq!((middle.suggestions[0].from, middle.suggestions[0].to), (0, 4));
}

#[test]
fn offsets_count_utf16_units_as_the_browser_does() {
    let text = "say h\u{e9}llo \u{1F600} ";
    assert_eq!(utf16_len(text), 13);
    assert_eq!(byte_index(text, 5), 5);
    assert_eq!(byte_index(text, 6), 7);
    assert_eq!(byte_index(text, 99), text.len());
    let after_emoji = "give \u{1F600} ";
    let completion = run(after_emoji);
    assert!(
        completion
            .suggestions
            .iter()
            .all(|s| s.from <= utf16_len(after_emoji))
    );
}

#[test]
fn unknown_commands_offer_nothing() {
    let completion = run("nope ");
    assert_eq!(completion.suggestions, []);
    assert!(completion.signature.is_none());
    assert_eq!(run("give @p diamond 1 extra").suggestions, []);
}

// ---- registries ----

#[test]
fn registries_keep_what_the_tree_uses() {
    let wanted = Registries::wanted(&TREE);
    assert!(wanted.contains("minecraft:enchantment") && wanted.contains("minecraft:mob_effect"));
    assert!(!wanted.contains("minecraft:chunk_status"));
    assert!(REGISTRIES.ids("minecraft:item").len() > 1_500);
    assert_eq!(REGISTRIES.ids("minecraft:chunk_status"), Vec::<String>::new());
    assert_eq!(
        REGISTRIES.ids("minecraft:enchantment"),
        ["minecraft:sharpness", "minecraft:smite"]
    );
    assert_eq!(REGISTRIES.tags("minecraft:item"), ["#minecraft:logs"]);
    assert_eq!(REGISTRIES.ids("minecraft:loot_table"), ["minecraft:blocks/stone"]);
}

#[test]
fn older_data_folders_were_plural() {
    let wanted: BTreeSet<String> = ["minecraft:block", "minecraft:advancement"]
        .map(str::to_owned)
        .into();
    let mut registries = Registries::default();
    registries.add_data_files(
        [
            "data/minecraft/tags/blocks/logs.json",
            "data/minecraft/advancements/story/root.json",
        ]
        .into_iter(),
        &wanted,
    );
    assert_eq!(registries.tags("minecraft:block"), ["#minecraft:logs"]);
    assert_eq!(registries.ids("minecraft:advancement"), ["minecraft:story/root"]);
}

fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[test]
fn ids_are_read_from_the_game_inside_a_bundler_jar_and_from_an_old_jar() {
    let wanted: BTreeSet<String> = ["minecraft:enchantment".to_owned()].into();
    let game = zip_of(&[
        ("data/minecraft/enchantment/mending.json", b"{}"),
        ("net/Main.class", b""),
    ]);
    let bundler = zip_of(&[
        ("META-INF/versions.list", b"x"),
        ("META-INF/versions/26.3/server-26.3.jar", &game),
    ]);
    let mut registries = Registries::default();
    registries
        .add_from_jar(std::io::Cursor::new(bundler), &wanted)
        .unwrap();
    assert_eq!(registries.ids("minecraft:enchantment"), ["minecraft:mending"]);

    let mut old = Registries::default();
    old.add_from_jar(std::io::Cursor::new(game), &wanted).unwrap();
    assert_eq!(old.ids("minecraft:enchantment"), ["minecraft:mending"]);
    assert!(
        Registries::default()
            .add_from_jar(std::io::Cursor::new(b"not a zip".to_vec()), &wanted)
            .is_err()
    );
}
