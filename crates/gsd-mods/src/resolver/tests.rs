#![allow(clippy::unwrap_used)]

use super::*;
use crate::model::{ContentFile, Dependency, SideSupport, SourceKind};

/// Projects by id, each with its versions (newest first).
#[derive(Default)]
struct FakeCatalog {
    projects: HashMap<String, (Project, Vec<ContentVersion>)>,
}

impl FakeCatalog {
    fn with(mut self, id: &str, versions: Vec<ContentVersion>) -> Self {
        let project = Project {
            source: SourceKind::Modrinth,
            id: id.into(),
            slug: id.into(),
            title: format!("{id} title"),
            description: String::new(),
            body: String::new(),
            icon_url: None,
            server_side: SideSupport::Optional,
            client_side: SideSupport::Optional,
            page_url: String::new(),
        };
        self.projects.insert(id.into(), (project, versions));
        self
    }
}

impl Catalog for FakeCatalog {
    async fn project(&self, id: &str) -> Result<Project, SourceError> {
        self.projects
            .get(id)
            .map(|(project, _)| project.clone())
            .ok_or(SourceError::NotFound)
    }

    async fn versions(&self, project_id: &str) -> Result<Vec<ContentVersion>, SourceError> {
        Ok(self
            .projects
            .get(project_id)
            .map(|(_, versions)| versions.clone())
            .unwrap_or_default())
    }
}

fn version(project: &str, id: &str, channel: Channel, dependencies: Vec<Dependency>) -> ContentVersion {
    ContentVersion {
        id: id.into(),
        project_id: project.into(),
        name: id.into(),
        version_number: id.into(),
        channel,
        published: None,
        game_versions: vec!["26.3".into()],
        loaders: vec!["fabric".into()],
        dependencies,
        file: ContentFile {
            url: format!("https://example.invalid/{id}.jar"),
            filename: format!("{id}.jar"),
            size: 1,
            sha1: None,
            sha256: None,
            sha512: None,
            manual_url: None,
        },
    }
}

fn requires(project: &str) -> Dependency {
    Dependency {
        project_id: Some(project.into()),
        name: None,
        version_id: None,
        kind: DependencyKind::Required,
    }
}

fn depends(project: &str, version: &str, kind: DependencyKind) -> Dependency {
    Dependency {
        project_id: Some(project.into()),
        name: None,
        version_id: Some(version.into()),
        kind,
    }
}

fn steps(plan: &InstallPlan) -> Vec<(&str, &str, Option<&str>)> {
    plan.steps
        .iter()
        .map(|step| {
            (
                step.project.id.as_str(),
                step.version.id.as_str(),
                step.required_by.as_deref(),
            )
        })
        .collect()
}

#[tokio::test]
async fn installs_required_dependencies_transitively_but_not_optional_ones() {
    let catalog = FakeCatalog::default()
        .with(
            "arch",
            vec![version(
                "arch",
                "arch-2",
                Channel::Release,
                vec![requires("api"), depends("menu", "m1", DependencyKind::Optional)],
            )],
        )
        .with(
            "api",
            vec![version(
                "api",
                "api-9",
                Channel::Release,
                vec![requires("kotlin")],
            )],
        )
        .with(
            "kotlin",
            vec![version("kotlin", "kt-1", Channel::Release, vec![])],
        )
        .with("menu", vec![version("menu", "m1", Channel::Release, vec![])]);

    let plan = plan(&catalog, &Installed::new(), "arch", None).await.unwrap();

    assert_eq!(
        steps(&plan),
        [
            ("arch", "arch-2", None),
            ("api", "api-9", Some("arch title")),
            ("kotlin", "kt-1", Some("api title"))
        ]
    );
}

#[tokio::test]
async fn installed_dependencies_stay_and_the_requested_project_replaces_its_old_file() {
    let catalog = FakeCatalog::default()
        .with(
            "skin",
            vec![version("skin", "skin-3", Channel::Release, vec![requires("api")])],
        )
        .with("api", vec![version("api", "api-9", Channel::Release, vec![])]);
    let installed = Installed::from([
        (
            "api".into(),
            InstalledVersion {
                filename: "api-8.jar".into(),
                version_id: Some("api-8".into()),
            },
        ),
        (
            "skin".into(),
            InstalledVersion {
                filename: "skin-2.jar".into(),
                version_id: Some("skin-2".into()),
            },
        ),
    ]);

    let plan = plan(&catalog, &installed, "skin", None).await.unwrap();

    assert_eq!(steps(&plan), [("skin", "skin-3", None)]);
    assert_eq!(plan.steps[0].replaces.as_deref(), Some("skin-2.jar"));
    assert_eq!(plan.already_installed, ["api title"]);
}

#[tokio::test]
async fn picks_the_newest_release_then_anything_and_explains_when_nothing_fits() {
    let catalog = FakeCatalog::default()
        .with(
            "mixed",
            vec![
                version("mixed", "beta-2", Channel::Beta, vec![]),
                version("mixed", "rel-1", Channel::Release, vec![]),
            ],
        )
        .with("betas", vec![version("betas", "alpha-1", Channel::Alpha, vec![])])
        .with("none", vec![]);

    assert_eq!(
        steps(&plan(&catalog, &Installed::new(), "mixed", None).await.unwrap())[0].1,
        "rel-1"
    );
    assert_eq!(
        steps(
            &plan(&catalog, &Installed::new(), "mixed", Some("beta-2"))
                .await
                .unwrap()
        )[0]
        .1,
        "beta-2",
        "a choice is respected"
    );
    assert_eq!(
        steps(&plan(&catalog, &Installed::new(), "betas", None).await.unwrap())[0].1,
        "alpha-1"
    );
    let error = plan(&catalog, &Installed::new(), "none", None).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "none title has no version for this server's Minecraft version and loader"
    );
    let error = plan(&catalog, &Installed::new(), "mixed", Some("gone"))
        .await
        .unwrap_err();
    assert!(matches!(error, ResolveError::UnknownVersion(_)), "{error}");
}

#[tokio::test]
async fn a_dependency_pinned_to_a_version_that_does_not_fit_gets_the_best_fit() {
    let catalog = FakeCatalog::default()
        .with(
            "mod",
            vec![version(
                "mod",
                "mod-1",
                Channel::Release,
                vec![depends("api", "api-for-26.2", DependencyKind::Required)],
            )],
        )
        .with("api", vec![version("api", "api-9", Channel::Release, vec![])]);

    assert_eq!(
        steps(&plan(&catalog, &Installed::new(), "mod", None).await.unwrap())[1].1,
        "api-9"
    );
}

#[tokio::test]
async fn incompatible_projects_stop_the_plan() {
    let catalog = FakeCatalog::default()
        .with(
            "optimizer",
            vec![version(
                "optimizer",
                "o-1",
                Channel::Release,
                vec![depends("other", "x", DependencyKind::Incompatible)],
            )],
        )
        .with(
            "bundle",
            vec![version(
                "bundle",
                "b-1",
                Channel::Release,
                vec![requires("optimizer"), requires("other")],
            )],
        )
        .with("other", vec![version("other", "x", Channel::Release, vec![])]);
    let installed = Installed::from([(
        "other".into(),
        InstalledVersion {
            filename: "other.jar".into(),
            version_id: None,
        },
    )]);

    let error = plan(&catalog, &installed, "optimizer", None).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "optimizer title can't be installed together with other.jar"
    );
    let error = plan(&catalog, &Installed::new(), "bundle", None)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "optimizer title can't be installed together with other title"
    );
}

#[tokio::test]
async fn dependency_cycles_end() {
    let catalog = FakeCatalog::default()
        .with(
            "a",
            vec![version("a", "a-1", Channel::Release, vec![requires("b")])],
        )
        .with(
            "b",
            vec![version("b", "b-1", Channel::Release, vec![requires("a")])],
        );

    assert_eq!(
        steps(&plan(&catalog, &Installed::new(), "a", None).await.unwrap()).len(),
        2
    );
}

/// Two mods that both need `api` (which needs `kotlin`), and `api` in two versions.
fn shared_catalog() -> FakeCatalog {
    FakeCatalog::default()
        .with(
            "skin",
            vec![version("skin", "skin-1", Channel::Release, vec![requires("api")])],
        )
        .with(
            "arch",
            vec![version("arch", "arch-1", Channel::Release, vec![requires("api")])],
        )
        .with(
            "api",
            vec![
                version("api", "api-9", Channel::Release, vec![requires("kotlin")]),
                version("api", "api-8", Channel::Release, vec![requires("kotlin")]),
            ],
        )
        .with(
            "kotlin",
            vec![version("kotlin", "kt-1", Channel::Release, vec![])],
        )
}

fn asked(project_id: &str, version_id: Option<&str>) -> Requested {
    Requested {
        source: SourceKind::Modrinth,
        project_id: project_id.into(),
        version_id: version_id.map(Into::into),
    }
}

#[tokio::test]
async fn several_asked_for_together_share_their_dependencies() {
    let plan = plan_many(
        &shared_catalog(),
        &Installed::new(),
        &[asked("skin", None), asked("arch", None)],
    )
    .await
    .unwrap();
    assert_eq!(
        steps(&plan),
        [
            ("skin", "skin-1", None),
            ("arch", "arch-1", None),
            ("api", "api-9", Some("skin title")),
            ("kotlin", "kt-1", Some("api title"))
        ],
        "api once, for whoever needed it first"
    );
}

#[tokio::test]
async fn a_dependency_asked_for_as_well_comes_as_asked_for() {
    let plan = plan_many(
        &shared_catalog(),
        &Installed::new(),
        &[asked("skin", None), asked("api", Some("api-8"))],
    )
    .await
    .unwrap();
    assert_eq!(
        steps(&plan),
        [
            ("skin", "skin-1", None),
            ("api", "api-8", None),
            ("kotlin", "kt-1", Some("api title"))
        ],
        "the chosen version, not the newest"
    );
}

#[tokio::test]
async fn an_installed_dependency_of_several_is_kept_and_named_once() {
    let installed = Installed::from([(
        "api".to_owned(),
        InstalledVersion {
            filename: "api-9.jar".into(),
            version_id: Some("api-9".into()),
        },
    )]);
    let plan = plan_many(
        &shared_catalog(),
        &installed,
        &[asked("skin", None), asked("arch", None)],
    )
    .await
    .unwrap();
    assert_eq!(steps(&plan), [("skin", "skin-1", None), ("arch", "arch-1", None)]);
    assert_eq!(plan.already_installed, ["api title"]);
}

/// What the server has from elsewhere: the file `api.jar`, going by the name "API title".
fn api_from_elsewhere() -> Installed {
    let mut installed = Installed::new();
    installed.add_untracked("api.jar", &["API title", "other-id"]);
    installed
}

#[tokio::test]
async fn a_file_from_elsewhere_with_the_same_name_counts_as_the_dependency() {
    let plan = plan(&shared_catalog(), &api_from_elsewhere(), "skin", None)
        .await
        .unwrap();
    assert_eq!(steps(&plan), [("skin", "skin-1", None)], "no second copy");
    assert_eq!(plan.already_installed, ["api title"]);
}

#[tokio::test]
async fn asking_for_a_project_replaces_its_file_from_elsewhere() {
    let plan = plan(&shared_catalog(), &api_from_elsewhere(), "api", None)
        .await
        .unwrap();
    assert_eq!(plan.steps[0].replaces.as_deref(), Some("api.jar"));
}

#[tokio::test]
async fn names_only_match_files_the_source_does_not_track() {
    // Two projects on one source may share a name; ids tell them apart there.
    let mut installed = Installed::new();
    installed.add_tracked(
        "fork".into(),
        InstalledVersion {
            filename: "fork.jar".into(),
            version_id: None,
        },
        &["api title"],
    );
    let plan = plan(&shared_catalog(), &installed, "api", None).await.unwrap();
    assert_eq!(plan.steps[0].replaces, None);
}

#[tokio::test]
async fn what_an_earlier_source_brings_satisfies_a_dependency_but_not_a_request() {
    let catalog = shared_catalog();
    let mut installed = Installed::new();
    installed.add_planned_elsewhere(&catalog.project("api").await.unwrap());

    let plan_skin = plan(&catalog, &installed, "skin", None).await.unwrap();
    assert_eq!(steps(&plan_skin), [("skin", "skin-1", None)]);

    let error = plan(&catalog, &installed, "api", None).await.unwrap_err();
    assert!(matches!(error, ResolveError::FromTwoSources(_)), "{error}");
}

#[tokio::test]
async fn a_required_dependency_hosted_elsewhere_is_named_unless_it_is_there() {
    let hosted_elsewhere = |required| Dependency {
        project_id: None,
        name: Some("Vault".into()),
        version_id: None,
        kind: if required {
            DependencyKind::Required
        } else {
            DependencyKind::Optional
        },
    };
    let catalog = FakeCatalog::default().with(
        "shop",
        vec![version(
            "shop",
            "shop-1",
            Channel::Release,
            vec![hosted_elsewhere(true), hosted_elsewhere(false)],
        )],
    );
    let missing = plan(&catalog, &Installed::new(), "shop", None).await.unwrap();
    assert_eq!(
        missing.not_available,
        [UnavailableDependency {
            name: "Vault".into(),
            required_by: "shop title".into()
        }],
        "the required one, once"
    );

    let mut installed = Installed::new();
    installed.add_untracked("Vault.jar", &["Vault"]);
    let there = plan(&catalog, &installed, "shop", None).await.unwrap();
    assert_eq!(there.not_available, []);
}

#[tokio::test]
async fn dependencies_of_an_update_come_as_dependencies() {
    let plan = plan_required(
        &shared_catalog(),
        &api_from_elsewhere(),
        vec![
            RequiredDependency {
                project_id: "api".into(),
                version_id: None,
                required_by: "skin title".into(),
            },
            RequiredDependency {
                project_id: "kotlin".into(),
                version_id: None,
                required_by: "skin title".into(),
            },
        ],
    )
    .await
    .unwrap();
    assert_eq!(
        steps(&plan),
        [("kotlin", "kt-1", Some("skin title"))],
        "what is there is kept"
    );
}
