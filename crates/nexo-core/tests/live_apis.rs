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

    // Every library must survive the download-host and path checks, and
    // Fabric's Maven libraries should publish digests.
    for lib in &active {
        if let Some(a) = lib.artifact() {
            assert!(
                nexo_core::util::https_host_allowed(
                    &a.url,
                    nexo_core::minecraft::meta::LIBRARY_HOSTS
                ),
                "{}: host not allowed: {}",
                lib.name,
                a.url
            );
            assert!(
                nexo_core::minecraft::meta::is_safe_relative(&a.path),
                "{}",
                a.path
            );
        } else if lib.url.is_some() {
            assert!(lib.maven_url().is_some(), "{} rejected", lib.name);
            // Digests are published for most Maven libraries but not all
            // (fabric-loader itself had none), so they are verified only
            // when present.
            println!("{}: sha1 published = {}", lib.name, lib.sha1.is_some());
        }
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

/// Adoptium's asset schema, against the real API.
///
/// Every field in `java::adoptium` is a guess about someone else's JSON, and
/// this one decides whether a machine with no Java can launch the game at all.
/// A rename upstream would otherwise surface as a failed launch rather than a
/// failing test.
#[tokio::test]
#[ignore = "hits the network"]
async fn adoptium_publishes_a_runtime_for_this_platform() {
    use nexo_core::java::MIN_JAVA_MAJOR;

    let paths = temp_paths();
    let http = reqwest::Client::new();

    // Nothing is installed under a fresh root, so this exercises the whole
    // path bar the download itself.
    assert!(
        nexo_core::java::adoptium::installed(&paths, MIN_JAVA_MAJOR)
            .await
            .is_none(),
        "an empty runtimes directory must not report an installed JVM"
    );

    let (os, arch) = match std::env::consts::OS {
        "linux" => ("linux", "x64"),
        "windows" => ("windows", "x64"),
        "macos" => ("mac", "x64"),
        other => panic!("no Adoptium mapping for {other}"),
    };
    let url = format!(
        "https://api.adoptium.net/v3/assets/latest/{MIN_JAVA_MAJOR}/hotspot\
         ?architecture={arch}&image_type=jre&os={os}&vendor=eclipse"
    );

    let assets: serde_json::Value = http
        .get(&url)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .expect("Adoptium should answer")
        .json()
        .await
        .unwrap();

    let first = assets
        .as_array()
        .and_then(|a| a.first())
        .unwrap_or_else(|| {
            panic!("Adoptium publishes no Java {MIN_JAVA_MAJOR} JRE for {os}-{arch}")
        });

    let package = &first["binary"]["package"];
    for field in ["name", "link", "checksum"] {
        assert!(
            package[field].as_str().is_some_and(|v| !v.is_empty()),
            "binary.package.{field} is missing — java::adoptium reads it: {package}"
        );
    }
    assert!(
        first["release_name"]
            .as_str()
            .is_some_and(|v| !v.is_empty()),
        "release_name is missing; it names the install directory"
    );

    // The archive format decides which extractor runs.
    let name = package["name"].as_str().unwrap();
    let expected = if cfg!(windows) { ".zip" } else { ".tar.gz" };
    assert!(
        name.ends_with(expected),
        "expected a {expected} for {os}, got {name}"
    );
}

/// The whole auto-install path, for real: query, download, verify, extract,
/// and run the result.
///
/// Downloads ~45 MB, so it is heavier than the rest of this file even by
/// `--ignored` standards. It earns that by being the only thing that proves
/// the extracted runtime is actually executable — on Unix that hinges on tar
/// preserving the mode bits, which no amount of parsing can tell you.
#[tokio::test]
#[ignore = "downloads a ~45 MB JRE"]
async fn a_runtime_can_be_installed_and_then_run() {
    use nexo_core::java::{MIN_JAVA_MAJOR, adoptium};

    let paths = temp_paths();
    let http = reqwest::Client::new();

    let install = adoptium::ensure(&http, &paths, MIN_JAVA_MAJOR, None)
        .await
        .expect("a JRE should install");

    assert!(
        install.major >= MIN_JAVA_MAJOR,
        "installed Java {} is below the floor of {MIN_JAVA_MAJOR}",
        install.major
    );
    assert!(
        install.path.starts_with(paths.java_runtimes()),
        "the runtime landed outside the managed directory: {}",
        install.path.display()
    );

    // Asking again must reuse it rather than downloading a second copy.
    let again = adoptium::installed(&paths, MIN_JAVA_MAJOR)
        .await
        .expect("the freshly installed runtime should be found");
    assert_eq!(again.path, install.path);

    std::fs::remove_dir_all(paths.root()).ok();
}

/// The SRV path, against servers that only work through it.
///
/// `hypixel.net` resolves to a web host with 25565 closed and `minehut.gg` has
/// no A record at all — both exist purely as `_minecraft._tcp` records. Before
/// SRV resolution they reported as unreachable in the server list, which is
/// what this guards against coming back.
#[tokio::test]
#[ignore = "hits the network"]
async fn srv_only_servers_answer_a_ping() {
    for address in ["hypixel.net", "minehut.gg"] {
        let status = nexo_core::server_ping::ping(address)
            .await
            .unwrap_or_else(|err| panic!("{address} should answer: {err}"));

        assert!(
            status.players_max > 0,
            "{address} reported no player slots: {status:?}"
        );
        println!(
            "{address}: {} — {}/{} online, {} ms",
            status.motd, status.players_online, status.players_max, status.latency_ms
        );
    }
}

/// An explicit port must suppress the SRV lookup, the way the game does it.
/// Port 25565 on `hypixel.net` is closed, so naming it has to fail even though
/// the bare name succeeds above.
#[tokio::test]
#[ignore = "hits the network"]
async fn an_explicit_port_bypasses_srv() {
    assert!(
        nexo_core::server_ping::ping("hypixel.net:25565")
            .await
            .is_err(),
        "an explicit port should have gone straight to a closed port"
    );
}

/// e4mc's broker and relay answer the quiclime handshake with a public domain.
#[tokio::test]
#[ignore]
async fn e4mc_relay_assigns_a_domain() {
    let http = reqwest::Client::new();
    let tunnel = nexo_core::hosting::tunnel::open(&http, 25565)
        .await
        .expect("tunnel should open");
    assert!(tunnel.domain.contains('.'), "domain: {}", tunnel.domain);
    println!("assigned {}", tunnel.domain);
    tunnel.close();
}

/// The whole hosting path against the real Paper/Modrinth/e4mc services and a
/// real JVM: make a server with a plugin, start it, run a command, open the
/// public address, stop it, delete it. Several minutes; needs Java 25+.
#[tokio::test]
#[ignore]
async fn hosting_end_to_end() {
    use nexo_core::hosting::{ServerKind, ServerSpec, WorldSource};
    let root = std::env::temp_dir().join(format!("nexo-host-{}", uuid::Uuid::new_v4()));
    let core = nexo_core::Nexo::with_paths(nexo_core::Paths::with_root(&root)).await.unwrap();
    let host = &core.hosting;
    // A folder whose record is unreadable must not be claimed — or deleted.
    let squatter = root.join("servers").join("live-test");
    std::fs::create_dir_all(&squatter).unwrap();
    std::fs::write(squatter.join("keep.txt"), "mine").unwrap();

    let created = host
        .create(ServerSpec {
            name: "Live Test".into(),
            kind: ServerKind::Paper,
            minecraft_version: "1.21.4".into(),
            world: WorldSource::New {
                seed: "42".into(),
                level_type: "minecraft:flat".into(),
                game_mode: "creative".into(),
                difficulty: "peaceful".into(),
                hardcore: false,
            },
            memory_mb: 2048,
            tunnel: true,
            // Slugs, as the Essentials list sends them. Neither runs alone:
            // ViaBackwards needs ViaVersion, WorldGuard needs WorldEdit.
            content: vec!["viabackwards".into(), "worldguard".into()],
            eula_accepted: true,
        })
        .await
        .expect("create");
    assert!(created.warnings.is_empty(), "{:?}", created.warnings);
    let id = created.server.id.clone();
    assert_eq!(id, "live-test-2");
    assert!(squatter.join("keep.txt").exists());
    let plugins: Vec<String> = host
        .server_dir(&id)
        .join("plugins")
        .read_dir()
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .map(|n| n.to_lowercase())
        .filter(|n| n.ends_with(".jar"))
        .collect();
    for wanted in ["viabackwards", "viaversion", "worldguard", "worldedit"] {
        assert!(plugins.iter().any(|p| p.contains(wanted)), "{wanted} missing from {plugins:?}");
    }
    println!("plugins: {plugins:?}");

    host.start(&id).await.expect("start");
    let ready = host.wait_ready(&id, None).await;
    let console = host.console(&id).await.unwrap_or_default();
    ready.unwrap_or_else(|e| panic!("ready: {e}\n{console}"));

    let players = host.players(&id).await.expect("list");
    assert_eq!(players.online, 0);
    assert!(players.max > 0);
    let props = host.properties(&id).await.unwrap();
    assert!(props.iter().any(|(k, v)| k == "level-type" && v == "minecraft:flat"));

    let domain = host.open_tunnel(&id).await.expect("tunnel");
    println!("public address {domain}");
    // A real player's first packets, from outside, through the relay.
    let status = nexo_core::server_ping::ping(&domain).await.expect("ping through the tunnel");
    assert_eq!(status.motd, "Live Test");
    println!("pinged through tunnel: {} ({}/{})", status.motd, status.players_online, status.players_max);
    let server = host.read(&id).await.unwrap();
    assert!(host.is_running(&server));
    // A launcher restarted since: nothing in its own registry, so only the
    // recorded pid plus the working-directory check can recognise it.
    let fresh = nexo_core::Nexo::with_paths(nexo_core::Paths::with_root(&root)).await.unwrap();
    assert!(fresh.hosting.is_running(&server));
    let mut stranger = server.clone();
    stranger.id = "live-test".into();
    assert!(!fresh.hosting.is_running(&stranger), "a pid must not match another server's folder");
    let sample = host.sample(&[server.clone()]);
    assert!(sample.get(&id).is_some_and(|s| s.memory_mb > 100));

    host.stop(&id).await.expect("stop");
    assert!(!host.is_running(&host.read(&id).await.unwrap()));
    host.delete(&id).await.expect("delete");
    let _ = std::fs::remove_dir_all(&root);
}



/// The update Play runs: an older tracked jar is replaced by the newest release's,
/// in the same edition, and a jar swapped in by hand is left alone.
#[tokio::test]
#[ignore = "hits the network"]
async fn nexo_mod_updates_itself_but_not_a_hand_copied_jar() {
    let paths = temp_paths();
    // GitHub answers 403 without a User-Agent.
    let http = reqwest::Client::builder()
        .user_agent(concat!("Lokifisch/nexo-client/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap();
    let nexo = NexoMod::new(http, paths.clone());
    let release = nexo.latest_including_prereleases().await.unwrap();

    let mut instance = Instance::new("x", &release.manifest.minecraft_version, Loader::Fabric);
    nexo.install(&mut instance, &release, Edition::Legit).await.unwrap();
    assert!(!nexo.update_if_due(&mut instance, None).await.unwrap(), "already current");

    instance.mods[0].version_number = "0.0.1".to_string();
    assert!(nexo.update_if_due(&mut instance, None).await.unwrap(), "older, so it updates");
    assert_eq!(instance.mods[0].version_number, release.manifest.mod_version);
    assert_eq!(nexo_core::nexo_mod::installed_edition(&instance), Some(Edition::Legit));

    instance.mods[0].version_number = "0.0.1".to_string();
    let jar = paths.instance_mods(&instance.id).join(&instance.mods[0].file_name);
    std::fs::write(&jar, b"my own build").unwrap();
    assert!(!nexo.update_if_due(&mut instance, None).await.unwrap(), "someone's own jar stays");
    assert_eq!(std::fs::read(&jar).unwrap(), b"my own build");

    // Gone, say after an interrupted update: the next Play brings it back.
    std::fs::remove_file(&jar).unwrap();
    assert!(nexo.update_if_due(&mut instance, None).await.unwrap());
    assert!(jar.exists());

    // A build newer than the release is left where it is.
    instance.mods[0].version_number = "999.0.0".to_string();
    assert!(!nexo.update_if_due(&mut instance, None).await.unwrap(), "never backwards");

    std::fs::remove_dir_all(paths.root()).unwrap();
}
