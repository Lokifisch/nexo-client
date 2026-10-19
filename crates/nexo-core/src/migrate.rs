//! Moves an existing instance to another Minecraft version in place.
//!
//! Worlds upgrade one way — once a newer version opens a save it can't go
//! back — so `saves/` is copied aside before anything else changes. Then the
//! version is switched, every Modrinth project is re-resolved for it, and
//! Nexo Mod follows its own releases. A mod with no build for the new version
//! is disabled rather than deleted: it stays in the list, ready to re-enable
//! once its author catches up.

use crate::content::ProjectKind;
use crate::error::{Error, Result};
use crate::instance::ModSource;
use std::path::PathBuf;

/// What a migration did, for the UI to show.
#[derive(Debug, Clone, Default)]
pub struct MigrationReport {
    /// Where the pre-migration `saves/` was copied, if there was anything to copy.
    pub backup: Option<PathBuf>,
    /// Projects moved to a build for the new version.
    pub updated: Vec<String>,
    /// Mods with no build for the new version, now disabled.
    pub disabled: Vec<String>,
    /// Hand-added files, left exactly as they were.
    pub untouched: Vec<String>,
}

impl crate::Nexo {
    pub async fn migrate_instance(
        &self,
        instance_id: &str,
        to_version: &str,
    ) -> Result<MigrationReport> {
        if self.running.is_running(instance_id) {
            return Err(Error::invalid(
                "close the game before migrating this instance",
            ));
        }
        let mut instance = self.instances.get(instance_id).await?;
        if instance.game_version == to_version {
            return Err(Error::invalid(format!(
                "'{}' is already on {to_version}",
                instance.name
            )));
        }
        let from_version = instance.game_version.clone();
        let mut report = MigrationReport::default();

        instance.game_version = to_version.to_string();
        // Re-resolved on the next install, against the new game version.
        instance.loader_version = None;
        // Before anything touches disk: the version can come from a release
        // manifest, and it ends up in paths and URLs.
        instance.validate(None)?;

        let dir = self.paths.instance(&instance.id);
        let saves = dir.join("saves");
        if std::fs::read_dir(&saves).is_ok_and(|mut d| d.next().is_some()) {
            let mut backup = dir.join(format!("saves-before-{from_version}"));
            if backup.exists() {
                backup = dir.join(format!(
                    "saves-before-{from_version}-{}",
                    crate::instance::now()
                ));
            }
            crate::paper_server::convert::copy_tree(&saves, &backup).await?;
            report.backup = Some(backup);
        }

        let nexo_release = if instance.mods.iter().any(|m| m.source == ModSource::NexoMod) {
            self.nexo_mod.latest_including_prereleases().await.ok()
        } else {
            None
        };
        for installed in instance.mods.clone() {
            match installed.source {
                ModSource::Local => report.untouched.push(installed.name),
                ModSource::NexoMod => {
                    let edition = crate::nexo_mod::installed_edition(&instance).unwrap_or_default();
                    let moved = match &nexo_release {
                        Some(release) if release.manifest.supports(&instance) => self
                            .nexo_mod
                            .install(&mut instance, release, edition)
                            .await
                            .is_ok(),
                        _ => false,
                    };
                    if moved {
                        report.updated.push(installed.name);
                    } else {
                        self.content
                            .set_enabled(&mut instance, &installed.project_id, false)
                            .await?;
                        report.disabled.push(installed.name);
                    }
                }
                ModSource::Modrinth => {
                    let kind = ProjectKind::ALL
                        .into_iter()
                        .find(|k| {
                            let folder = dir.join(k.folder());
                            folder.join(&installed.file_name).exists()
                                || folder
                                    .join(format!("{}.disabled", installed.file_name))
                                    .exists()
                        })
                        .unwrap_or(ProjectKind::Mod);
                    match self
                        .content
                        .install_modrinth(&mut instance, &installed.project_id, kind)
                        .await
                    {
                        Ok(()) => {
                            if !installed.enabled {
                                self.content
                                    .set_enabled(&mut instance, &installed.project_id, false)
                                    .await?;
                            }
                            report.updated.push(installed.name);
                        }
                        // Resource packs and shaders mostly survive a version
                        // bump; only a mod built for the old version is a crash.
                        Err(_) if kind != ProjectKind::Mod => report.untouched.push(installed.name),
                        Err(_) => {
                            self.content
                                .set_enabled(&mut instance, &installed.project_id, false)
                                .await?;
                            report.disabled.push(installed.name);
                        }
                    }
                }
            }
        }

        self.instances.save(&instance).await?;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use crate::instance::{InstalledMod, Loader, ModSource};
    use crate::{Nexo, Paths};

    #[tokio::test]
    async fn backs_up_saves_and_leaves_local_files_alone() {
        let root = std::env::temp_dir().join(format!("nexo-migrate-test-{}", uuid::Uuid::new_v4()));
        let nexo = Nexo::with_paths(Paths::with_root(&root)).await.unwrap();
        let mut inst = nexo
            .instances
            .create("Old", "26.1.2", Loader::Fabric)
            .await
            .unwrap();
        inst.loader_version = Some("0.19.3".into());
        inst.mods.push(InstalledMod {
            project_id: "local:mine".into(),
            name: "Mine".into(),
            version_id: String::new(),
            version_number: String::new(),
            file_name: "mine.jar".into(),
            source: ModSource::Local,
            enabled: true,
            edition: None,
        });
        nexo.instances.save(&inst).await.unwrap();
        let dir = root.join("instances").join(&inst.id);
        std::fs::create_dir_all(dir.join("saves/World")).unwrap();
        std::fs::write(dir.join("saves/World/level.dat"), b"x").unwrap();
        std::fs::write(dir.join("mods/mine.jar"), b"jar").unwrap();

        let report = nexo.migrate_instance(&inst.id, "26.3").await.unwrap();

        assert_eq!(
            report.backup.as_deref(),
            Some(dir.join("saves-before-26.1.2").as_path())
        );
        assert!(dir.join("saves-before-26.1.2/World/level.dat").exists());
        assert_eq!(report.untouched, ["Mine"]);
        assert!(dir.join("mods/mine.jar").exists());
        let migrated = nexo.instances.get(&inst.id).await.unwrap();
        assert_eq!(migrated.game_version, "26.3");
        assert_eq!(migrated.loader_version, None);
        assert!(
            nexo.migrate_instance(&inst.id, "26.3").await.is_err(),
            "already on it"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
