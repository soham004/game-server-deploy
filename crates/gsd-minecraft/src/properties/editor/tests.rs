use chrono::Utc;
use gsd_core::{
    ids::InstanceId,
    instances::{
        InstanceDefinition, InstanceLayout, Ports, SCHEMA_VERSION, SandboxIds,
        definition::{BackupPolicy, Behavior, JavaSettings, JvmPreset, LaunchSpec, RuntimeChoice},
    },
};

use super::*;
use crate::test_support::fixture_text;

/// The file a real Minecraft 26.3 server generated on first start (2026-11-07), port 25565.
fn generated() -> String {
    fixture_text("server_26.3.properties")
}

fn instance(dir: &std::path::Path) -> Instance {
    let layout = InstanceLayout::new(dir.join("survival"));
    std::fs::create_dir_all(layout.server_dir()).unwrap();
    Instance {
        definition: InstanceDefinition {
            schema: SCHEMA_VERSION,
            id: InstanceId::generate(),
            name: "Survival".into(),
            game: "minecraft".into(),
            created_at: Utc::now(),
            installed_at: Some(Utc::now()),
            server: toml::Table::new(),
            launch: LaunchSpec::Jar {
                jar: "server.jar".into(),
                args: vec![],
            },
            java: JavaSettings {
                major: 25,
                runtime: RuntimeChoice::Auto,
                min_memory_mb: 1024,
                max_memory_mb: 2048,
                preset: JvmPreset::None,
                extra_jvm_args: vec![],
            },
            behavior: Behavior::default(),
            backups: BackupPolicy::default(),
            ports: Ports { game: 25565 },
            sandbox: SandboxIds {
                uid: 20000,
                gid: 20000,
            },
        },
        layout,
    }
}

fn with_file(text: &str) -> (tempfile::TempDir, Instance) {
    let dir = tempfile::tempdir().unwrap();
    let instance = instance(dir.path());
    std::fs::write(path(&instance), text).unwrap();
    (dir, instance)
}

fn changes(pairs: &[(&str, Option<&str>)]) -> PropertiesEdit {
    PropertiesEdit::Changes(
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.map(str::to_owned)))
            .collect(),
    )
}

fn on_disk(instance: &Instance) -> String {
    std::fs::read_to_string(path(instance)).unwrap()
}

fn error_keys(error: PropertiesError) -> Vec<String> {
    match error {
        PropertiesError::Invalid(errors) => errors.into_iter().map(|e| e.key).collect(),
        other => panic!("expected a validation error, got {other:?}"),
    }
}

#[test]
fn every_key_of_a_real_26_3_file_is_in_the_schema_with_its_real_default() {
    let doc = PropertiesDoc::parse(&generated());
    let entries = doc.entries();
    assert!(entries.len() > 50, "{}", entries.len());
    for (key, value) in entries {
        let schema = schema::find(key).unwrap_or_else(|| panic!("{key} missing from the schema"));
        let managed_or_secret = schema.managed || key == "rcon.password" || key == "management-server-secret";
        if !managed_or_secret {
            assert_eq!(schema.default, value, "default of {key}");
        }
    }
}

#[tokio::test]
async fn reading_a_missing_file_gives_an_empty_document() {
    let dir = tempfile::tempdir().unwrap();
    let snapshot = read(&instance(dir.path())).await.unwrap();
    assert!(!snapshot.exists);
    assert_eq!(snapshot.text, "");
    assert_eq!(snapshot.values, Vec::<KeyValue>::new());
    assert_eq!(snapshot.revision, revision(b""));
}

#[tokio::test]
async fn changes_touch_only_their_lines() {
    let (_dir, instance) = with_file(&generated());
    let before = read(&instance).await.unwrap();
    let after = apply(
        &instance,
        &changes(&[("motd", Some("Friends \u{a7}aSMP")), ("max-players", Some("50"))]),
        &before.revision,
    )
    .await
    .unwrap();

    assert_ne!(after.revision, before.revision);
    let expected = generated()
        .replace("motd=A Minecraft Server", "motd=Friends \\u00a7aSMP")
        .replace("max-players=20", "max-players=50");
    assert_eq!(on_disk(&instance), expected);
    assert_eq!(after.text, expected);
    let motd = after.values.iter().find(|kv| kv.key == "motd").unwrap();
    assert_eq!(motd.value, "Friends \u{a7}aSMP");
    assert!(!path(&instance).with_extension("properties.gsd-tmp").exists());
}

#[tokio::test]
async fn a_stale_revision_is_refused_and_nothing_is_written() {
    let (_dir, instance) = with_file(&generated());
    let before = read(&instance).await.unwrap();
    // The server rewrote the file meanwhile.
    std::fs::write(path(&instance), generated() + "written-by-the-server=1\n").unwrap();
    let result = apply(&instance, &changes(&[("motd", Some("x"))]), &before.revision).await;
    assert!(matches!(result, Err(PropertiesError::Conflict)), "{result:?}");
    assert!(on_disk(&instance).contains("motd=A Minecraft Server"));
}

#[tokio::test]
async fn invalid_values_are_reported_per_key() {
    let (_dir, instance) = with_file(&generated());
    let revision = read(&instance).await.unwrap().revision;
    let error = apply(
        &instance,
        &changes(&[
            ("difficulty", Some("Hard")),
            ("max-players", Some("lots")),
            ("view-distance", Some("100")),
            ("hardcore", Some("yes")),
            ("motd", Some("fine")),
        ]),
        &revision,
    )
    .await
    .unwrap_err();
    let mut keys = error_keys(error);
    keys.sort();
    assert_eq!(keys, ["difficulty", "hardcore", "max-players", "view-distance"]);
    assert_eq!(on_disk(&instance), generated(), "nothing written");
}

#[tokio::test]
async fn app_managed_keys_can_not_be_changed_or_removed() {
    let (_dir, instance) = with_file(&generated());
    let revision = read(&instance).await.unwrap().revision;
    for edit in [
        changes(&[("server-port", Some("25599"))]),
        changes(&[("server-port", None)]),
        changes(&[("rcon.port", Some("25576"))]),
        PropertiesEdit::Replace(generated().replace("query.port=25565", "query.port=1")),
    ] {
        let error = apply(&instance, &edit, &revision).await.unwrap_err();
        assert!(!error_keys(error).is_empty(), "{edit:?}");
    }
    // Sending the current value is fine.
    apply(&instance, &changes(&[("server-port", Some("25565"))]), &revision)
        .await
        .unwrap();
}

#[tokio::test]
async fn the_raw_editor_writes_exactly_what_was_typed() {
    let (_dir, instance) = with_file(&generated());
    let revision = read(&instance).await.unwrap().revision;
    let typed = generated().replace(
        "motd=A Minecraft Server",
        "# a note\nmotd = Typed by hand\nmy-plugin.option=anything",
    );
    let after = apply(&instance, &PropertiesEdit::Replace(typed.clone()), &revision)
        .await
        .unwrap();
    assert_eq!(on_disk(&instance), typed);
    assert!(
        after
            .values
            .iter()
            .any(|kv| kv.key == "my-plugin.option" && kv.value == "anything")
    );
}

#[tokio::test]
async fn unknown_keys_are_free_text() {
    let (_dir, instance) = with_file("server-port=25565\n");
    let revision = read(&instance).await.unwrap().revision;
    apply(
        &instance,
        &changes(&[("some-plugin-setting", Some("anything goes")), ("", Some("x"))]),
        &revision,
    )
    .await
    .unwrap_err();
    apply(
        &instance,
        &changes(&[("some-plugin-setting", Some("anything goes"))]),
        &revision,
    )
    .await
    .unwrap();
    assert_eq!(
        on_disk(&instance),
        "server-port=25565\nsome-plugin-setting=anything goes\n"
    );
}

#[tokio::test]
async fn oversized_files_are_refused() {
    let (_dir, instance) = with_file("server-port=25565\n");
    let revision = read(&instance).await.unwrap().revision;
    let huge = format!("server-port=25565\nmotd={}\n", "x".repeat(MAX_FILE_BYTES));
    let result = apply(&instance, &PropertiesEdit::Replace(huge), &revision).await;
    assert!(matches!(result, Err(PropertiesError::TooLarge)));
}

#[tokio::test]
async fn redaction_hides_secrets_in_values_and_text() {
    let text = generated().replace("rcon.password=", "rcon.password=hunter2");
    let (_dir, instance) = with_file(&text);
    let full = read(&instance).await.unwrap();
    assert!(full.text.contains("hunter2"));
    let redacted = full.redacted();
    assert!(!redacted.text.contains("hunter2"));
    assert!(!redacted.values.iter().any(|kv| kv.value.contains("hunter2")));
    assert_eq!(redacted.revision, full.revision);
}
