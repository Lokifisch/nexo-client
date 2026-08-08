//! Contract tests against the real upstream APIs.
//!
//! These exist because every struct in `minecraft::meta`, `minecraft::fabric`,
//! and `modrinth` is a guess about someone else's JSON until it's been parsed
//! against the live service. A schema drift upstream would otherwise surface
//! as a launch failure in the UI rather than a test failure here.
//!
//! Network-dependent, so they're `#[ignore]`d by default:
//!
//! ```sh
//! cargo test -p nexo-core --test live_apis -- --ignored --nocapture
//! ```

use nexo_core::instance::{Instance, Loader};
use nexo_core::minecraft::{fabric, Installer};
use nexo_core::modrinth::{Modrinth, SearchQuery};
use nexo_core::paths::Paths;

fn temp_paths() -> Paths {
    Paths::with_root(std::env::temp_dir().join(format!("nexo-live-{}", uuid::Uuid::new_v4())))
}

fn installer() -> Installer {
    Installer::new(reqwest::Client::new(), temp_paths())
}

#[tokio::test]
#[ignore = "hits the network"]
async fn mojang_manifest_parses_and_has_releases() {
    let manifest = installer().version_manifest().await.unwrap();

    assert!(!manifest.latest.release.is_empty());
    let releases: Vec<_> = manifest.releases().collect();
    assert!(
        releases.len() > 50,
        "expected a substantial release list, got {}",
        releases.len()
    );

    println!("latest release: {}", manifest.latest.release);
    println!("latest snapshot: {}", manifest.latest.snapshot);
    println!(
        "newest 5 releases: {:?}",
        releases.iter().take(5).map(|v| &v.id).collect::<Vec<_>>()
    );

    assert!(
        manifest.find(&manifest.latest.release).is_some(),
        "the latest release should be findable by id"
    );
}

/// The whole point of `merge_onto`: a Fabric profile plus vanilla has to yield
/// a launchable description — Fabric's main class, vanilla's assets, and both
/// sets of libraries.
#[tokio::test]
#[ignore = "hits the network"]
async fn fabric_profile_merges_onto_vanilla() {
    let installer = installer();
    let manifest = installer.version_manifest().await.unwrap();
    let game_version = manifest.latest.release.clone();

    let loader = fabric::latest_stable(&reqwest::Client::new(), &game_version)
        .await
        .unwrap();
    println!("resolved fabric loader {loader} for MC {game_version}");

    let instance = Instance::new("live-test", &game_version, Loader::Fabric);
    let version = installer.resolve(&instance).await.unwrap();

    assert!(
        version.main_class.contains("fabric") || version.main_class.contains("knot"),
        "expected Fabric's main class, got {}",
        version.main_class
    );
    assert!(
        version.asset_index.is_some(),
        "asset index must be inherited from vanilla"
    );
    assert!(
        version.client_download().is_some(),
        "client jar download must be inherited from vanilla"
    );

    let active: Vec<_> = version.active_libraries().collect();
    assert!(
        active.len() > 10,
        "expected a real library set, got {}",
        active.len()
    );
    // Fabric's own libraries carry no `downloads` block, so the Maven
    // fallback path has to resolve for them.
    let fabric_libs: Vec<_> = active
        .iter()
        .filter(|l| l.name.starts_with("net.fabricmc"))
        .collect();
    assert!(!fabric_libs.is_empty(), "no fabric libraries in merged set");
    for lib in &fabric_libs {
        assert!(
            lib.artifact().is_some() || lib.maven_url().is_some(),
            "{} is not resolvable to a URL",
            lib.name
        );
    }

    println!(
        "merged: main_class={} libraries={} assets={}",
        version.main_class,
        active.len(),
        version.asset_index.as_ref().unwrap().id
    );
}

#[tokio::test]
#[ignore = "hits the network"]
async fn modrinth_search_and_versions_parse() {
    let modrinth = Modrinth::new().unwrap();

    let results = modrinth
        .search(&SearchQuery {
            text: "sodium",
            loader: Some("fabric"),
            ..Default::default()
        })
        .await
        .unwrap();

    assert!(!results.hits.is_empty(), "search returned nothing");
    println!(
        "top hits: {:?}",
        results.hits.iter().take(3).map(|h| &h.title).collect::<Vec<_>>()
    );

    let project = modrinth.project("sodium").await.unwrap();
    assert_eq!(project.slug, "sodium");
    assert!(!project.game_versions.is_empty());

    // Pick a version the project genuinely supports, so this doesn't fail
    // whenever the newest Minecraft outpaces the mod.
    let game_version = project.game_versions.last().unwrap().clone();
    let version = modrinth
        .latest_version(&project.id, "fabric", &game_version)
        .await
        .unwrap();

    let file = version.primary_file().unwrap();
    assert!(file.filename.ends_with(".jar"));
    assert!(file.hashes.sha1.is_some(), "expected a sha1 to verify against");
    println!(
        "sodium {} for {game_version}: {} ({} bytes)",
        version.version_number, file.filename, file.size
    );
}
