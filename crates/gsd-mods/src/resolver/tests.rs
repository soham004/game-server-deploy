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
            sha512: None,
        },
    }
}

fn requires(project: &str) -> Dependency {
    Dependency {
        project_id: Some(project.into()),
        version_id: None,
        kind: DependencyKind::Required,
    }
}

fn depends(project: &str, version: &str, kind: DependencyKind) -> Dependency {
    Dependency {
        project_id: Some(project.into()),
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
