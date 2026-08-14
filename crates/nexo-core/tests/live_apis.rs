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
use nexo_core::minecraft::{Installer, fabric};
use nexo_core::modrinth::{Modrinth, SearchQuery};
use nexo_core::nexo_mod::{Edition, NexoMod};
use nexo_core::paths::Paths;
use nexo_core::self_update::{self, SelfUpdate};

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
        results
            .hits
            .iter()
            .take(3)
            .map(|h| &h.title)
            .collect::<Vec<_>>()
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
    assert!(
        file.hashes.sha1.is_some(),
        "expected a sha1 to verify against"
    );
    println!(
        "sodium {} for {game_version}: {} ({} bytes)",
        version.version_number, file.filename, file.size
    );
}

/// Writes a store for the Java interop check. Not a test of its own — it
/// produces the fixture that `AccountStore.java` is then run against, which
/// is the only way to prove the two implementations agree on the format.
///
/// ```sh
/// NEXO_INTEROP_ROOT=/tmp/nexo-interop \
///   cargo test -p nexo-core --test live_apis interop -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "fixture generator for the Java interop check"]
async fn interop_fixture() {
    use nexo_core::auth::{Account, SkinModel};
    use nexo_core::shared_store::{Contents, SharedStore};

    let root = std::env::var("NEXO_INTEROP_ROOT").expect("NEXO_INTEROP_ROOT must be set");
    let path = std::path::Path::new(&root)
        .join("nexo")
        .join("accounts.dat");

    let contents = Contents {
        accounts: vec![
            Account {
                uuid: "069a79f444e94726a5befca90e38aaf5".into(),
                username: "AlphaPlayer".into(),
                access_token: "mc-token-alpha".into(),
                refresh_token: "msa-refresh-alpha".into(),
                expires_at: 1_900_000_000,
                skin_url: Some("https://example.invalid/alpha.png".into()),
                skin_model: SkinModel::Slim,
                cape_url: Some("https://example.invalid/cape.png".into()),
            },
            Account {
                uuid: "853c80ef3c3749fdaa49938b674adae6".into(),
                username: "BetaPlayer".into(),
                access_token: "mc-token-beta".into(),
                refresh_token: "msa-refresh-beta".into(),
                expires_at: 1_900_000_001,
                skin_url: None,
                skin_model: SkinModel::Classic,
                cape_url: None,
            },
        ],
        active: Some("853c80ef3c3749fdaa49938b674adae6".into()),
        ..Contents::default()
    };

    SharedStore::new(&path).save(&contents).await.unwrap();
    println!("WROTE {}", path.display());
}

/// Reads back a store the Java side wrote, completing the round trip.
#[tokio::test]
#[ignore = "second half of the Java interop check"]
async fn interop_read_back() {
    use nexo_core::shared_store::SharedStore;

    let root = std::env::var("NEXO_INTEROP_ROOT").expect("NEXO_INTEROP_ROOT must be set");
    let path = std::path::Path::new(&root)
        .join("nexo")
        .join("accounts.dat");

    let contents = SharedStore::new(&path).load().await.unwrap();
    for account in &contents.accounts {
        println!("  {} uuid={}", account.username, account.uuid);
    }
    println!("ACTIVE={:?}", contents.active);

    // The account Java added must be visible here, with its dashes stripped
    // back to the form the rest of the crate keys on.
    assert!(
        contents
            .accounts
            .iter()
            .any(|a| a.username == "GammaFromGame" && a.uuid == "11111111222233334444555555555555"),
        "the account written by Java was not readable from Rust"
    );
    // And the ones Rust wrote earlier must have survived Java's rewrite.
    assert!(
        contents
            .accounts
            .iter()
            .any(|a| a.username == "AlphaPlayer")
    );
    assert_eq!(contents.accounts.len(), 3);
}

/// The edition table in a release's `manifest.json` is the same species of
/// assumption as the upstream APIs above: it is JSON published by something
/// outside this binary, and a release that omits or misspells an edition key
/// would surface as "install did nothing" in the UI rather than as a failure
/// here.
///
/// It also pins the property the whole split rests on. Both jars ship in one
/// release, so resolution must come from the declared file name — "the first
/// .jar asset" is a coin flip between an edition a server allows and one it
/// bans.
#[tokio::test]
#[ignore = "hits the network"]
async fn nexo_release_resolves_both_editions_by_declared_file_name() {
    // The app's client, not a bare one: GitHub answers 403 to a request with
    // no User-Agent, so a bare client would fail here for a reason the real
    // launcher never hits.
    let http = reqwest::Client::builder()
        .user_agent(concat!("Lokifisch/nexo-client/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap();
    let nexo = NexoMod::new(http, temp_paths());
    let release = nexo.latest_including_prereleases().await.unwrap();

    assert!(
        release.offers_a_choice(),
        "{} publishes only one edition; the picker would have nothing to offer",
        release.version()
    );

    for edition in Edition::ALL {
        let build = release
            .edition(edition)
            .unwrap_or_else(|| panic!("{edition} is not published in {}", release.version()));

        assert!(
            build.jar_name.ends_with(".jar") && !build.jar_name.ends_with("-sources.jar"),
            "{edition} resolved to {}, which is not an installable jar",
            build.jar_name
        );
    }

    // The distinguishing property: the two editions are different artifacts.
    let tactical = release.edition(Edition::Tactical).unwrap();
    let legit = release.edition(Edition::Legit).unwrap();
    assert_ne!(tactical.jar_name, legit.jar_name);
    assert_ne!(tactical.mod_id, legit.mod_id);
}

/// The launcher's own update check against the real repository.
///
/// `Lokifisch/nexo-client` has published no release yet, so the assertion that
/// matters today is that "no releases" resolves to *up to date* rather than to
/// an error — an empty repo must not put a permanent failure in the sidebar.
/// Once a release exists this keeps checking the same contract from the other
/// side: whatever it returns has to be newer than the running build.
#[tokio::test]
#[ignore = "hits the network"]
async fn self_update_check_survives_a_repo_with_no_releases() {
    let http = reqwest::Client::builder()
        .user_agent(concat!("Lokifisch/nexo-client/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap();

    let found = SelfUpdate::new(http)
        .check()
        .await
        .expect("a repo with no releases is 'up to date', not an error");

    let current = semver::Version::parse(self_update::CURRENT).unwrap();
    match found {
        None => {}
        Some(update) => {
            assert!(
                update.version > current,
                "offered {} while running {current} — the check must never downgrade",
                update.version
            );
            assert!(
                update.url.contains(&self_update::asset_name().unwrap()),
                "the download URL doesn't point at this platform's build: {}",
                update.url
            );
        }
    }
}
