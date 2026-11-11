//! Schedules API and engine: saving and validating plans, time and event triggers, conditions,
//! restart countdowns, backups, catching up after downtime, and whose permissions a schedule
//! runs with. Servers are the fake Java from `gsd_minecraft::test_support`; times are a second
//! or two ahead, so these tests run in real time.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use chrono::{TimeDelta, Utc};
use common::{TestApp, json_body};
use gsd_core::{
    auth::{GlobalRole, Grant, Permission, PermissionSet},
    ids::InstanceId,
    scheduler::Scheduler,
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

fn url(id: &str) -> String {
    format!("/api/v1/instances/{id}/schedules")
}

fn plan(name: &str, trigger: Value, actions: Value) -> Value {
    json!({ "name": name, "enabled": true, "trigger": trigger, "actions": actions })
}

fn in_seconds(secs: i64) -> Value {
    json!({ "kind": "once", "at": Utc::now() + TimeDelta::seconds(secs) })
}

async fn create(app: &TestApp, id: &str, plan: Value, cookie: &str) -> Value {
    let response = app.post(&url(id), plan, Some(cookie)).await;
    let status = response.status();
    let body = json_body(response).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body
}

/// Waits until the schedule's newest run has finished; returns it.
async fn finished_run(app: &TestApp, id: &str, schedule: &str, cookie: &str) -> Value {
    for _ in 0..400 {
        let runs = json_body(
            app.get(&format!("{}/{schedule}/runs", url(id)), Some(cookie))
                .await,
        )
        .await;
        if let Some(run) = runs.as_array().and_then(|runs| runs.first())
            && run["outcome"] != "running"
        {
            return run.clone();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("schedule {schedule} never finished a run");
}

async fn console(app: &TestApp, id: &str, cookie: &str) -> Vec<String> {
    let snapshot = json_body(
        app.get(&format!("/api/v1/instances/{id}/console"), Some(cookie))
            .await,
    )
    .await;
    snapshot["console"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line["text"].as_str().unwrap().to_owned())
        .collect()
}

async fn start(app: &TestApp, id: &str, cookie: &str) {
    let response = app
        .post(&format!("/api/v1/instances/{id}/start"), json!({}), Some(cookie))
        .await;
    assert!(response.status().is_success());
    app.wait_for_state(id, cookie, "running").await;
}

#[tokio::test]
async fn saves_describes_edits_and_deletes_schedules() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Planned", &cookie).await;
    let nightly = plan(
        "Nightly restart",
        json!({ "kind": "cron", "expr": "0 4 * * *", "time_zone": "Europe/London" }),
        json!([{ "kind": "restart", "warn_secs": [600, 60] }, { "kind": "backup", "scope": "world" }]),
    );

    let created = create(&app, &id, nightly.clone(), &cookie).await;
    assert_eq!(created["plan"]["name"], "Nightly restart");
    assert_eq!(created["run_as"], "owner");
    assert!(
        created["description"].as_str().unwrap().contains("04:00"),
        "{created}"
    );
    let next = created["next_run_at"].as_str().unwrap().to_owned();
    assert!(next > Utc::now().to_rfc3339(), "{next}");
    let schedule = created["id"].as_str().unwrap();

    // Renaming keeps the next time; disabling clears it.
    let mut renamed = nightly.clone();
    renamed["name"] = json!("  Nightly  ");
    let updated = json_body(
        app.put(
            &format!("{}/{schedule}", url(&id)),
            renamed.clone(),
            Some(&cookie),
        )
        .await,
    )
    .await;
    assert_eq!(
        (updated["plan"]["name"].as_str(), updated["next_run_at"].as_str()),
        (Some("Nightly"), Some(next.as_str()))
    );
    renamed["enabled"] = json!(false);
    let disabled = json_body(
        app.put(&format!("{}/{schedule}", url(&id)), renamed, Some(&cookie))
            .await,
    )
    .await;
    assert_eq!(disabled["next_run_at"], Value::Null);

    let listed = json_body(app.get(&url(&id), Some(&cookie)).await).await;
    assert_eq!(listed["schedules"].as_array().unwrap().len(), 1);
    let response = app
        .delete(&format!("{}/{schedule}", url(&id)), Some(&cookie))
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        json_body(app.get(&url(&id), Some(&cookie)).await).await["schedules"],
        json!([])
    );
}

#[tokio::test]
async fn invalid_plans_are_refused_with_a_reason() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Strict", &cookie).await;
    let say = json!([{ "kind": "command", "command": "say hi" }]);
    let cases = [
        plan(
            "Bad cron",
            json!({ "kind": "cron", "expr": "every day", "time_zone": "UTC" }),
            say.clone(),
        ),
        plan(
            "Bad zone",
            json!({ "kind": "cron", "expr": "0 4 * * *", "time_zone": "Moon/Base" }),
            say.clone(),
        ),
        plan(
            "Too often",
            json!({ "kind": "interval", "every_secs": 5 }),
            say.clone(),
        ),
        plan(
            "Past",
            json!({ "kind": "once", "at": "2020-01-01T00:00:00Z" }),
            say.clone(),
        ),
        plan("Nothing to do", in_seconds(60), json!([])),
        plan(
            "Two lines",
            in_seconds(60),
            json!([{ "kind": "command", "command": "say a\nop me" }]),
        ),
        plan(
            "A carriage return ends a line too",
            in_seconds(60),
            json!([{ "kind": "command", "command": "say a\rop me" }]),
        ),
        // The name is shown in the console beside every command the schedule sends.
        plan("Nightly] > op me\n[owner", in_seconds(60), say.clone()),
    ];
    for body in cases {
        let response = app.post(&url(&id), body.clone(), Some(&cookie)).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(
            json_body(response).await["error"]["code"],
            "invalid_schedule",
            "{body}"
        );
    }
    let mut unknown = plan("Typo", in_seconds(60), say);
    unknown["actoins"] = json!([]);
    let response = app.post(&url(&id), unknown, Some(&cookie)).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn a_one_off_schedule_sends_its_commands_and_records_the_run() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Chatty", &cookie).await;
    start(&app, &id, &cookie).await;

    let actions = json!([
        { "kind": "broadcast", "message": "hello from a schedule" },
        { "kind": "command", "command": "/say the slash is optional" },
    ]);
    let created = create(&app, &id, plan("Greeting", in_seconds(1), actions), &cookie).await;
    let schedule = created["id"].as_str().unwrap();

    let run = finished_run(&app, &id, schedule, &cookie).await;
    assert_eq!(
        (run["outcome"].as_str(), run["cause"].as_str()),
        (Some("succeeded"), Some("trigger")),
        "{run}"
    );
    let lines = console(&app, &id, &cookie).await;
    assert!(
        lines
            .iter()
            .any(|l| l == "[schedule Greeting] > say hello from a schedule"),
        "{lines:#?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l == "[schedule Greeting] > say the slash is optional"),
        "{lines:#?}"
    );
    let listed = json_body(app.get(&url(&id), Some(&cookie)).await).await;
    let saved = &listed["schedules"][0];
    assert_eq!(saved["next_run_at"], Value::Null, "a one-off runs once");
    assert_eq!(saved["last_run"]["outcome"], "succeeded");
}

#[tokio::test]
async fn unmet_conditions_skip_a_run_but_run_now_ignores_them() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Picky", &cookie).await;
    let mut picky = plan(
        "Backup if running",
        in_seconds(1),
        json!([{ "kind": "backup", "scope": "world", "note": "scheduled" }]),
    );
    picky["conditions"] = json!([{ "kind": "server_running" }]);
    let schedule = create(&app, &id, picky, &cookie).await["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let skipped = finished_run(&app, &id, &schedule, &cookie).await;
    assert_eq!(skipped["outcome"], "skipped");
    assert_eq!(skipped["message"], "Skipped: the server wasn't running.");

    let response = app
        .post(&format!("{}/{schedule}/run", url(&id)), json!({}), Some(&cookie))
        .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let ran = finished_run(&app, &id, &schedule, &cookie).await;
    assert_eq!(
        (ran["outcome"].as_str(), ran["cause"].as_str()),
        (Some("succeeded"), Some("manual")),
        "{ran}"
    );
    let backups = json_body(
        app.get(&format!("/api/v1/instances/{id}/backups"), Some(&cookie))
            .await,
    )
    .await;
    assert_eq!(backups["backups"][0]["kind"], "scheduled");
    assert_eq!(backups["backups"][0]["note"], "scheduled");
}

#[tokio::test]
async fn an_event_trigger_runs_when_the_server_starts() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Welcoming", &cookie).await;
    let welcome = plan(
        "Welcome",
        json!({ "kind": "event", "event": { "type": "started" } }),
        json!([{ "kind": "broadcast", "message": "the server is back" }]),
    );
    let schedule = create(&app, &id, welcome, &cookie).await["id"]
        .as_str()
        .unwrap()
        .to_owned();

    start(&app, &id, &cookie).await;

    let run = finished_run(&app, &id, &schedule, &cookie).await;
    assert_eq!(run["outcome"], "succeeded", "{run}");
    assert!(
        console(&app, &id, &cookie)
            .await
            .iter()
            .any(|l| l == "[schedule Welcome] > say the server is back")
    );
}

#[tokio::test]
async fn a_restart_warns_the_players_first_then_restarts() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Restarting", &cookie).await;
    start(&app, &id, &cookie).await;
    let before =
        json_body(app.get(&format!("/api/v1/instances/{id}"), Some(&cookie)).await).await["state"]["since"]
            .clone();

    let restart = plan(
        "Restart",
        in_seconds(1),
        json!([{ "kind": "restart", "warn_secs": [2, 1] }]),
    );
    let schedule = create(&app, &id, restart, &cookie).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let run = finished_run(&app, &id, &schedule, &cookie).await;

    assert_eq!(run["outcome"], "succeeded", "{run}");
    let lines = console(&app, &id, &cookie).await;
    let position = |text: &str| {
        lines
            .iter()
            .position(|l| l.ends_with(text))
            .unwrap_or_else(|| panic!("{text:?} in {lines:#?}"))
    };
    assert!(position("say Restarting in 2 seconds") < position("say Restarting in 1 second"));
    let after = json_body(app.get(&format!("/api/v1/instances/{id}"), Some(&cookie)).await).await;
    assert_eq!(
        after["state"]["status"], "running",
        "it waits until the server is up again"
    );
    assert_ne!(after["state"]["since"], before, "a new process");
}

/// Someone who stops the server while a restart counts down wants it stopped. The restart then
/// has nothing to restart, and must not start it.
#[tokio::test]
async fn a_restart_does_not_start_a_server_that_was_stopped_during_its_countdown() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Stopped meanwhile", &cookie).await;
    start(&app, &id, &cookie).await;

    let restart = plan(
        "Restart",
        in_seconds(1),
        json!([{ "kind": "restart", "warn_secs": [3] }]),
    );
    let schedule = create(&app, &id, restart, &cookie).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for _ in 0..200 {
        if console(&app, &id, &cookie)
            .await
            .iter()
            .any(|line| line.ends_with("say Restarting in 3 seconds"))
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let stopped = app
        .post(&format!("/api/v1/instances/{id}/stop"), json!({}), Some(&cookie))
        .await;
    assert_eq!(stopped.status(), StatusCode::ACCEPTED);

    let run = finished_run(&app, &id, &schedule, &cookie).await;
    assert_eq!(run["outcome"], "succeeded", "{run}");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let state = json_body(app.get(&format!("/api/v1/instances/{id}"), Some(&cookie)).await).await;
    assert_eq!(state["state"]["status"], "stopped", "{state}");
}

/// "Run now" uses the permissions of whoever saved the schedule. Someone who may manage schedules
/// but not, say, stop the server must not get it stopped by pressing the button on a schedule
/// that an admin made.
#[tokio::test]
async fn running_a_schedule_by_hand_needs_the_right_to_do_what_it_does() {
    let app = TestApp::new().await;
    let owner = app.setup_owner().await;
    let id = app.installed_instance("Shared", &owner).await;
    let instance: InstanceId = id.parse().unwrap();
    start(&app, &id, &owner).await;

    let stop = create(
        &app,
        &id,
        plan("Stop", in_seconds(3600), json!([{ "kind": "stop" }])),
        &owner,
    )
    .await;
    let stop = stop["id"].as_str().unwrap();
    let say = create(
        &app,
        &id,
        plan(
            "Say",
            in_seconds(3600),
            json!([{ "kind": "command", "command": "say hi" }]),
        ),
        &owner,
    )
    .await;
    let say = say["id"].as_str().unwrap();

    let planner = app.create_user("planner", GlobalRole::Member).await;
    let can =
        |permissions: &[Permission]| Grant::Custom(permissions.iter().copied().collect::<PermissionSet>());
    let grants = app.state.auth.grants();
    grants
        .set(
            planner,
            instance,
            can(&[
                Permission::InstanceView,
                Permission::SchedulesManage,
                Permission::ConsoleWrite,
            ]),
        )
        .await
        .unwrap();
    let planner_cookie = app.login("planner").await;

    let refused = app
        .post(
            &format!("{}/{stop}/run", url(&id)),
            json!({}),
            Some(&planner_cookie),
        )
        .await;
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    let body = json_body(refused).await;
    assert_eq!(body["error"]["code"], "missing_permission", "{body}");
    assert!(
        body["error"]["message"].as_str().unwrap().contains("lifecycle"),
        "{body}"
    );
    let runs = json_body(
        app.get(&format!("{}/{stop}/runs", url(&id)), Some(&planner_cookie))
            .await,
    )
    .await;
    assert_eq!(runs, json!([]), "nothing ran");
    let state = json_body(app.get(&format!("/api/v1/instances/{id}"), Some(&owner)).await).await;
    assert_eq!(state["state"]["status"], "running", "the server wasn't stopped");

    // What they could do themselves, they may run.
    let allowed = app
        .post(
            &format!("{}/{say}/run", url(&id)),
            json!({}),
            Some(&planner_cookie),
        )
        .await;
    assert_eq!(allowed.status(), StatusCode::ACCEPTED);
    let run = finished_run(&app, &id, say, &owner).await;
    assert_eq!(run["outcome"], "succeeded", "{run}");
    assert!(
        console(&app, &id, &owner)
            .await
            .iter()
            .any(|line| line == "[schedule Say] > say hi")
    );
}

#[tokio::test]
async fn a_run_is_skipped_while_another_operation_holds_the_server() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Busy", &cookie).await;
    // A running server that never confirms saves keeps a backup job busy for the save timeout.
    std::fs::write(app.state.config.runtimes_dir().join("temurin-25-x64/no-save"), "").unwrap();
    start(&app, &id, &cookie).await;
    let schedule = create(
        &app,
        &id,
        plan("Wait", in_seconds(3600), json!([{ "kind": "wait", "secs": 1 }])),
        &cookie,
    )
    .await;
    let schedule = schedule["id"].as_str().unwrap();

    let backup = app
        .post(
            &format!("/api/v1/instances/{id}/backups"),
            json!({ "scope": "world" }),
            Some(&cookie),
        )
        .await;
    assert_eq!(backup.status(), StatusCode::ACCEPTED);
    app.post(&format!("{}/{schedule}/run", url(&id)), json!({}), Some(&cookie))
        .await;

    let run = finished_run(&app, &id, schedule, &cookie).await;
    assert_eq!(run["outcome"], "skipped", "{run}");
    assert!(
        run["message"].as_str().unwrap().contains("another operation"),
        "{run}"
    );
}

#[tokio::test]
async fn schedules_run_with_their_authors_permissions_checked_at_save_and_at_run() {
    let app = TestApp::new().await;
    let owner = app.setup_owner().await;
    let id = app.installed_instance("Shared", &owner).await;
    let instance: InstanceId = id.parse().unwrap();
    let grants = app.state.auth.grants();

    let operator = app.create_user("operator", GlobalRole::Member).await;
    grants.set(operator, instance, Grant::Operator).await.unwrap();
    let operator_cookie = app.login("operator").await;
    let say = json!([{ "kind": "command", "command": "say hi" }]);
    let response = app
        .post(
            &url(&id),
            plan("Mine", in_seconds(3600), say.clone()),
            Some(&operator_cookie),
        )
        .await;
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "operators can't manage schedules"
    );

    let planner = app.create_user("planner", GlobalRole::Member).await;
    let can =
        |permissions: &[Permission]| Grant::Custom(permissions.iter().copied().collect::<PermissionSet>());
    grants
        .set(
            planner,
            instance,
            can(&[
                Permission::InstanceView,
                Permission::SchedulesManage,
                Permission::ConsoleWrite,
            ]),
        )
        .await
        .unwrap();
    let planner_cookie = app.login("planner").await;
    let restart = json!([{ "kind": "restart" }]);
    let response = app
        .post(
            &url(&id),
            plan("Restart", in_seconds(3600), restart),
            Some(&planner_cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(
        json_body(response).await["error"]["message"]
            .as_str()
            .unwrap()
            .contains("lifecycle")
    );
    let schedule = create(&app, &id, plan("Say", in_seconds(3600), say), &planner_cookie).await;
    assert_eq!(schedule["run_as"], "planner");
    let schedule = schedule["id"].as_str().unwrap();

    // The planner loses console access: the schedule fails rather than act beyond their rights.
    grants
        .set(
            planner,
            instance,
            can(&[Permission::InstanceView, Permission::SchedulesManage]),
        )
        .await
        .unwrap();
    app.post(&format!("{}/{schedule}/run", url(&id)), json!({}), Some(&owner))
        .await;
    let run = finished_run(&app, &id, schedule, &owner).await;
    assert_eq!(run["outcome"], "failed");
    assert!(
        run["message"]
            .as_str()
            .unwrap()
            .contains("planner no longer has the \"console.write\" permission"),
        "{run}"
    );
}

#[tokio::test]
async fn deleting_a_server_deletes_its_schedules() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Doomed", &cookie).await;
    create(
        &app,
        &id,
        plan("Later", in_seconds(3600), json!([{ "kind": "wait", "secs": 1 }])),
        &cookie,
    )
    .await;

    let response = app
        .delete(&format!("/api/v1/instances/{id}"), Some(&cookie))
        .await;
    assert!(response.status().is_success());

    let count = || async {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM schedules WHERE instance_id = ?")
            .bind(&id)
            .fetch_one(&app.pool)
            .await
            .unwrap()
    };
    for _ in 0..100 {
        if count().await == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the schedule outlived its server");
}

#[tokio::test]
async fn after_downtime_missed_schedules_catch_up_or_are_recorded_as_skipped() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Sleeper", &cookie).await;
    let wait = json!([{ "kind": "wait", "secs": 1 }]);
    let mut catch_up = plan(
        "Catch up",
        json!({ "kind": "interval", "every_secs": 3600 }),
        wait.clone(),
    );
    catch_up["catch_up"] = json!(true);
    let catch_up = create(&app, &id, catch_up, &cookie).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let skip = plan("Skip", json!({ "kind": "interval", "every_secs": 3600 }), wait);
    let skip = create(&app, &id, skip, &cookie).await["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // Both were due an hour ago, while "the app was off"; a freshly started engine finds them.
    let an_hour_ago = Utc::now() - TimeDelta::hours(1);
    sqlx::query("UPDATE schedules SET next_run_at = ?")
        .bind(an_hour_ago)
        .execute(&app.pool)
        .await
        .unwrap();
    let state = &app.state;
    let restarted = Scheduler::new(
        app.pool.clone(),
        state.instances.clone(),
        state.backups.clone(),
        state.auth.clone(),
        state.jobs.clone(),
        state.events.clone(),
    );
    let stop = CancellationToken::new();
    restarted.start(stop.clone());

    let caught_up = finished_run(&app, &id, &catch_up, &cookie).await;
    assert_eq!(
        (caught_up["outcome"].as_str(), caught_up["cause"].as_str()),
        (Some("succeeded"), Some("catch_up")),
        "{caught_up}"
    );
    let skipped = finished_run(&app, &id, &skip, &cookie).await;
    assert_eq!(skipped["outcome"], "skipped");
    assert!(
        skipped["message"]
            .as_str()
            .unwrap()
            .contains("while the app was off"),
        "{skipped}"
    );
    let listed = json_body(app.get(&url(&id), Some(&cookie)).await).await;
    for schedule in listed["schedules"].as_array().unwrap() {
        assert!(
            schedule["next_run_at"].as_str().unwrap() > Utc::now().to_rfc3339().as_str(),
            "{schedule}"
        );
    }
    stop.cancel();
}

#[tokio::test]
async fn somebody_played_since_the_last_run_is_tracked_from_the_console() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Played", &cookie).await;
    start(&app, &id, &cookie).await;
    let only_if_played = |secs| {
        let mut plan = plan(
            "If played",
            in_seconds(secs),
            json!([{ "kind": "wait", "secs": 1 }]),
        );
        plan["conditions"] =
            json!([{ "kind": "players_since_last_run" }, { "kind": "players_online", "min": 1 }]);
        plan
    };

    let nobody = create(&app, &id, only_if_played(1), &cookie).await;
    let run = finished_run(&app, &id, nobody["id"].as_str().unwrap(), &cookie).await;
    assert_eq!(run["message"], "Skipped: nobody played since the last run.");

    // The fake server reports "Steve joined the game", as Minecraft does.
    app.post(
        &format!("/api/v1/instances/{id}/command"),
        json!({ "command": "join Steve" }),
        Some(&cookie),
    )
    .await;
    let played = create(&app, &id, only_if_played(1), &cookie).await;
    let run = finished_run(&app, &id, played["id"].as_str().unwrap(), &cookie).await;
    assert_eq!(run["outcome"], "succeeded", "{run}");
}

#[tokio::test]
async fn a_time_window_skips_runs_outside_it() {
    let app = TestApp::new().await;
    let cookie = app.setup_owner().await;
    let id = app.installed_instance("Nightly only", &cookie).await;
    // A window that starts two hours from now: the run a second from now falls outside it.
    let now = Utc::now();
    let from = (now + TimeDelta::hours(2)).format("%H:%M:00").to_string();
    let to = (now + TimeDelta::hours(3)).format("%H:%M:00").to_string();
    let mut windowed = plan("Windowed", in_seconds(1), json!([{ "kind": "wait", "secs": 1 }]));
    windowed["conditions"] = json!([{ "kind": "time_window", "from": from, "to": to, "time_zone": "UTC" }]);

    let created = create(&app, &id, windowed, &cookie).await;
    assert_eq!(
        created["plan"]["conditions"][0]["from"],
        from.as_str(),
        "stored as written"
    );
    let run = finished_run(&app, &id, created["id"].as_str().unwrap(), &cookie).await;
    assert_eq!(run["outcome"], "skipped");
    assert!(run["message"].as_str().unwrap().contains("is outside"), "{run}");
}
