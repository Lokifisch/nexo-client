//! Moves an existing instance to a newer Minecraft version in place.
//!
//! Worlds upgrade one way — once a newer version opens a save it can't go
//! back — so `saves/`, `mods/` and the manifest are copied to `before-<old>/`
//! before anything changes; putting them back undoes the migration. Then the
//! version is switched, every Modrinth project is re-resolved for it, and
//! Nexo Mod follows its own releases. A mod with no build for the new version
//! — including every hand-added jar, which can't be checked — is disabled
//! rather than deleted: it stays in the list, ready to re-enable.

use crate::content::ProjectKind;
use crate::error::{Error, IoContext, Result};
use crate::instance::{Instance, Loader, ModSource};
use std::path::PathBuf;

/// What a migration did, for the UI to show.
#[derive(Debug, Clone, Default)]
pub struct MigrationReport {
    /// `before-<old version>/`: the pre-migration saves, mods and manifest.
    pub backup: Option<PathBuf>,
    /// Projects moved to a build for the new version.
    pub updated: Vec<String>,
    /// Mods with no build for the new version, or hand-added jars; now disabled.
    pub disabled: Vec<String>,
    /// Mods that couldn't be checked (offline, rate limit, …); disabled too,
    /// since the old jar would stop the game from starting.
    pub failed: Vec<String>,
    /// Resource packs and shaders with no new build, left as they were.
    pub untouched: Vec<String>,
}

impl crate::Nexo {
    /// Refuses anything but a real, newer Minecraft version the instance's
    /// loader supports — the target can come from a release manifest, and an
    /// older one would open the worlds in a version that corrupts them.
    pub async fn migrate_instance(
        &self,
        instance_id: &str,
        to_version: &str,
    ) -> Result<MigrationReport> {
        let instance = self.instances.get(instance_id).await?;
        let manifest = self.installer.version_manifest().await?;
        let position = |id: &str| manifest.versions.iter().position(|v| v.id == id);
        match (position(to_version), position(&instance.game_version)) {
            (Some(to), Some(from)) if to < from => {}
            (None, _) => {
                return Err(Error::invalid(format!(
                    "Minecraft {to_version} doesn't exist"
                )));
            }
            _ => {
                return Err(Error::invalid(format!(
                    "{to_version} isn't newer than {}; migrating can only upgrade",
                    instance.game_version
                )));
            }
        }
        if instance.loader == Loader::Fabric {
            crate::minecraft::fabric::latest_stable(&self.http, to_version)
                .await
                .map_err(|_| Error::invalid(format!("Fabric doesn't support {to_version} yet")))?;
        }
        self.migrate_unchecked(instance, to_version).await
    }

    pub(crate) async fn migrate_unchecked(
        &self,
        mut instance: Instance,
        to_version: &str,
    ) -> Result<MigrationReport> {
        if self.running.is_running(&instance.id) {
            return Err(Error::invalid(
                "close the game before migrating this instance",
            ));
        }
        let from_version = instance.game_version.clone();
        let mut report = MigrationReport::default();

        instance.game_version = to_version.to_string();
        // Re-resolved on the next install, against the new game version.
        instance.loader_version = None;
        // Before anything touches disk: the version ends up in paths and URLs.
        instance.validate(None)?;

        let dir = self.paths.instance(&instance.id);
        let mut backup = dir.join(format!("before-{from_version}"));
        if backup.exists() {
            backup = dir.join(format!("before-{from_version}-{}", crate::instance::now()));
        }
        for sub in ["saves", "mods"] {
            if dir.join(sub).is_dir() {
                crate::paper_server::convert::copy_tree(&dir.join(sub), &backup.join(sub)).await?;
            }
        }
        tokio::fs::create_dir_all(&backup).await.ctx(&backup)?;
        let manifest = dir.join(crate::instance::MANIFEST);
        tokio::fs::copy(&manifest, backup.join(crate::instance::MANIFEST))
            .await
            .ctx(&manifest)?;
        report.backup = Some(backup);

        // From here on nothing returns early: old jars are already being
        // replaced, so the manifest must be saved whatever happens.
        let nexo_release = if instance.mods.iter().any(|m| m.source == ModSource::NexoMod) {
            Some(self.nexo_mod.latest_including_prereleases().await)
        } else {
            None
        };
        for installed in instance.mods.clone() {
            let outcome: Result<Outcome> = async {
                match installed.source {
                    ModSource::Local => {
                        if installed.enabled && dir.join("mods").join(&installed.file_name).exists()
                        {
                            self.content
                                .set_enabled(&mut instance, &installed.project_id, false)
                                .await?;
                            Ok(Outcome::Disabled)
                        } else {
                            Ok(Outcome::Skipped)
                        }
                    }
                    ModSource::NexoMod => {
                        let release = match nexo_release.as_ref() {
                            Some(Ok(release)) => release,
                            Some(Err(e)) => return Err(Error::invalid(e.to_string())),
                            None => return Ok(Outcome::Skipped),
                        };
                        if !release.manifest.supports(&instance) {
                            return Ok(Outcome::NoBuild);
                        }
                        let edition =
                            crate::nexo_mod::installed_edition(&instance).unwrap_or_default();
                        self.nexo_mod
                            .install(&mut instance, release, edition)
                            .await?;
                        Ok(Outcome::Updated)
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
                                Ok(Outcome::Updated)
                            }
                            // Resource packs and shaders mostly survive a version
                            // bump; only a mod built for the old version is a crash.
                            Err(Error::Invalid(_)) if kind != ProjectKind::Mod => Ok(Outcome::Kept),
                            Err(Error::Invalid(_)) => Ok(Outcome::NoBuild),
                            Err(e) => Err(e),
                        }
                    }
                }
            }
            .await;

            let name = installed.name.clone();
            match outcome {
                Ok(Outcome::Updated) => report.updated.push(name),
                Ok(Outcome::Disabled) => report.disabled.push(name),
                Ok(Outcome::Kept) => report.untouched.push(name),
                Ok(Outcome::Skipped) => {}
                Ok(Outcome::NoBuild) => {
                    let _ = self
                        .content
                        .set_enabled(&mut instance, &installed.project_id, false)
                        .await;
                    report.disabled.push(name);
                }
                Err(e) => {
                    let _ = self
                        .content
                        .set_enabled(&mut instance, &installed.project_id, false)
                        .await;
                    report.failed.push(format!("{name} ({e})"));
                }
            }
        }

        // Jars dropped into mods/ by hand, which the manifest never heard of.
        let mods = dir.join("mods");
        if let Ok(entries) = std::fs::read_dir(&mods) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".jar") && !instance.mods.iter().any(|m| m.file_name == name) {
                    let renamed =
                        tokio::fs::rename(entry.path(), mods.join(format!("{name}.disabled")))
                            .await;
                    match renamed {
                        Ok(()) => report.disabled.push(name),
                        Err(e) => report.failed.push(format!("{name} ({e})")),
                    }
                }
            }
        }

        self.instances.save(&instance).await?;
        Ok(report)
    }
}

enum Outcome {
    Updated,
    Disabled,
    Kept,
    Skipped,
    NoBuild,
}

#[cfg(test)]
mod tests {
    use crate::instance::{InstalledMod, Loader, ModSource};
    use crate::{Nexo, Paths};

    #[tokio::test]
    async fn backs_up_and_disables_jars_it_cannot_update() {
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
        std::fs::write(dir.join("mods/stray.jar"), b"jar").unwrap();

        let report = nexo.migrate_unchecked(inst.clone(), "26.3").await.unwrap();

        let backup = dir.join("before-26.1.2");
        assert_eq!(report.backup.as_deref(), Some(backup.as_path()));
        assert!(backup.join("saves/World/level.dat").exists());
        assert!(backup.join("mods/mine.jar").exists());
        assert!(backup.join("nexo-instance.json").exists());
        assert_eq!(report.disabled, ["Mine", "stray.jar"]);
        assert!(report.failed.is_empty());
        assert!(dir.join("mods/mine.jar.disabled").exists());
        assert!(dir.join("mods/stray.jar.disabled").exists());
        let migrated = nexo.instances.get(&inst.id).await.unwrap();
        assert_eq!(migrated.game_version, "26.3");
        assert_eq!(migrated.loader_version, None);
        assert!(!migrated.mods[0].enabled);

        // A version that would escape the instance folder never gets as far as a backup.
        assert!(nexo.migrate_unchecked(migrated, "../x").await.is_err());
        assert!(!dir.join("before-26.3").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
