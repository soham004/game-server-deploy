#![allow(clippy::unwrap_used)]

use chrono::TimeDelta;

use super::*;
use crate::{
    auth::{
        GlobalRole, Username,
        users::{NewUser, UserRepo},
    },
    backups::BackupScope,
    scheduler::model::{Action, OnError, Trigger},
    store,
};

fn plan(name: &str) -> SchedulePlan {
    SchedulePlan {
        name: name.into(),
        enabled: true,
        trigger: Trigger::Interval { every_secs: 3600 },
        conditions: vec![],
        actions: vec![Action::Backup {
            scope: BackupScope::World,
            note: None,
        }],
        on_error: OnError::Stop,
        catch_up: true,
    }
}

async fn setup() -> (ScheduleRepo, UserRepo, UserId) {
    let pool = store::open_in_memory().await.unwrap();
    let users = UserRepo::new(pool.clone());
    let user = users
        .insert(&NewUser {
            username: Username::parse("owner").unwrap(),
            password_hash: "x".into(),
            role: GlobalRole::Owner,
            must_change_password: false,
        })
        .await
        .unwrap();
    (ScheduleRepo::new(pool), users, user.id)
}

fn schedule(instance: InstanceId, run_as: UserId, name: &str) -> Schedule {
    let now = Utc::now();
    Schedule {
        id: ScheduleId::generate(),
        instance_id: instance,
        plan: plan(name),
        run_as: Some(run_as),
        created_at: now,
        last_run_at: None,
        next_run_at: Some(now + TimeDelta::hours(1)),
    }
}

#[tokio::test]
async fn stores_the_plan_as_written_and_filters_by_instance() {
    let (repo, _, user) = setup().await;
    let (mine, other) = (InstanceId::generate(), InstanceId::generate());
    let first = schedule(mine, user, "First");
    repo.insert(&first).await.unwrap();
    repo.insert(&schedule(other, user, "Elsewhere")).await.unwrap();

    assert_eq!(repo.get(first.id).await.unwrap(), Some(first.clone()));
    let listed = repo.list(Some(mine)).await.unwrap();
    assert_eq!(listed, [first]);
    assert_eq!(repo.list(None).await.unwrap().len(), 2);
}

#[tokio::test]
async fn edits_take_over_the_schedule_and_times_are_recorded() {
    let (repo, users, owner) = setup().await;
    let admin = users
        .insert(&NewUser {
            username: Username::parse("admin").unwrap(),
            password_hash: "x".into(),
            role: GlobalRole::Admin,
            must_change_password: false,
        })
        .await
        .unwrap()
        .id;
    let original = schedule(InstanceId::generate(), owner, "Before");
    repo.insert(&original).await.unwrap();

    assert!(
        repo.update_plan(original.id, &plan("After"), admin, None)
            .await
            .unwrap()
    );
    let ran_at = Utc::now();
    repo.set_times(original.id, Some(ran_at), Some(ran_at + TimeDelta::hours(1)))
        .await
        .unwrap();

    let saved = repo.get(original.id).await.unwrap().unwrap();
    assert_eq!(saved.plan.name, "After");
    assert_eq!(saved.run_as, Some(admin), "whoever saved it last");
    assert_eq!(saved.last_run_at, Some(ran_at));
    assert!(
        !repo
            .update_plan(ScheduleId::generate(), &plan("x"), admin, None)
            .await
            .unwrap()
    );

    // The user is deleted: the schedule stays, without anyone to run as.
    users.delete(admin).await.unwrap();
    assert_eq!(repo.get(original.id).await.unwrap().unwrap().run_as, None);
}

#[tokio::test]
async fn records_runs_newest_first_and_keeps_only_the_latest() {
    let (repo, _, user) = setup().await;
    let schedule = schedule(InstanceId::generate(), user, "Busy");
    repo.insert(&schedule).await.unwrap();

    for n in 0..RUNS_KEPT + 5 {
        let run = repo
            .start_run(schedule.id, RunCause::Trigger, Utc::now())
            .await
            .unwrap();
        let outcome = if n % 2 == 0 {
            RunOutcome::Succeeded
        } else {
            RunOutcome::Skipped
        };
        repo.finish_run(run, outcome, Some(&format!("run {n}")), Utc::now())
            .await
            .unwrap();
    }
    let runs = repo.runs(schedule.id, 100).await.unwrap();
    assert_eq!(runs.len(), usize::try_from(RUNS_KEPT).unwrap());
    assert_eq!(
        runs[0].message.as_deref(),
        Some(format!("run {}", RUNS_KEPT + 4).as_str())
    );
    assert_eq!(runs[0].cause, RunCause::Trigger);

    // A run the app never finished is closed on startup.
    let open = repo
        .start_run(schedule.id, RunCause::Manual, Utc::now())
        .await
        .unwrap();
    assert_eq!(repo.close_interrupted_runs(Utc::now()).await.unwrap(), 1);
    let closed = &repo.runs(schedule.id, 1).await.unwrap()[0];
    assert_eq!((closed.id, closed.outcome), (open, RunOutcome::Failed));

    // Deleting the schedule deletes its runs.
    assert!(repo.delete(schedule.id).await.unwrap());
    assert_eq!(repo.runs(schedule.id, 100).await.unwrap(), []);
}
