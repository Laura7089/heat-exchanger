use crate::steam::{get_game_version, SteamVersion};
use bollard::Docker;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug)]
pub struct Container {
    pub name: String,
    pub appid: u64,
    #[serde(default)]
    current_version: SteamVersion,
    action: UpdateAction,
    #[serde(default)]
    options: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub enum UpdateAction {
    #[serde(rename = "build")]
    DockerBuild { context_path: PathBuf },
    #[serde(rename = "restart")]
    DockerRestart,
    #[serde(rename = "pull")]
    DockerPull { image: String, tag: String },
    #[serde(rename = "custom")]
    Custom { chdir: PathBuf, command: String },
}

impl Container {
    /// Initialise the container
    ///
    /// The `Container` instance must exist beforehand, since we assume that it is deserialised
    /// from the config file.
    ///
    /// This method will check if there is an existing save file; if there is, it will load the
    /// version from that and then check the version is up to date.
    /// If there isn't, it will assume the existing container is up to date, get the version
    /// number from steam and stop.
    pub async fn init(&mut self, key: &str, docker_client: &Docker, state_dir: &PathBuf) {
        debug!(
            "Initialising container {} (appid {})",
            self.name, self.appid
        );

        let save_path = self.get_save_path(state_dir);

        // Load in the saved version
        if save_path.exists() {
            info!(
                "Saved state for {} found at {}",
                self.name,
                save_path.display()
            );
            let content = match std::fs::read_to_string(&save_path) {
                Ok(c) => c,
                Err(e) => panic!("FAILED to read state file {}: {}", &save_path.display(), e),
            };
            let saved_version: Self = match serde_json::from_str(&content) {
                Ok(s) => s,
                Err(e) => panic!(
                    "FAILED to deserialise state from file {}: {}",
                    &save_path.display(),
                    e
                ),
            };
            self.current_version = saved_version.current_version;

            // Check the game is up-to-date now that we've initialised it
            debug!("Running initial update check for {}", self.name);
            self.update(key, docker_client).await;
        } else {
            match get_game_version(key, self.appid).await {
                Ok(v) => {
                    info!(
                        "Initialised container {} (appid {}): version {} found",
                        self.name, self.appid, v
                    );
                    self.current_version = v;
                }
                Err(e) => error!(
                    "FAILED to initialise container {} (appid {}): {}",
                    self.name, self.appid, e
                ),
            }
        }
    }

    /// Check for updates and carry them out on a container
    ///
    /// Checks for version changes via steam, and if the versions don't match, runs the relevant
    /// update handler for that container (restart, pull etc.)
    pub async fn update(&mut self, api_key: &str, docker_client: &Docker) {
        // Get the version integer from steam
        debug!("Checking version of {}", self.name);
        let new_version = match get_game_version(&api_key, self.appid).await {
            Ok(v) => {
                debug!(
                    "Got new version for container {} (appid {}): {}",
                    self.name, self.appid, v
                );
                v
            }
            Err(e) => {
                error!(
                    "FAILED to check version of container {} (appid {}): {}",
                    self.name, self.appid, e
                );
                return;
            }
        };

        // If our version matches, just log + return without further action
        if self.current_version == new_version {
            info!(
                "{} is UP-TO-DATE at version {}",
                self.name, self.current_version
            );
            return;
        }

        // Check the container is running, if not, warn and skip
        let container_running =
            match docker_client.inspect_container(&self.name, None).await {
                Ok(r) => {
                    if let Some(state) = r.state {
                        state.running == Some(true)
                    } else {
                        error!(
                            "FAILED inspecting container {}: no state returned by docker",
                            self.name
                        );
                        return;
                    }
                }
                Err(e) => {
                    error!("FAILED inspecting container {}: {}", self.name, e);
                    return;
                }
            };
        if !container_running {
            warn!(
                "Container {} not running, skipping update action",
                self.name
            );
            return;
        }

        // Otherwise, start our update action and update the version tag if the update completes
        // successfully
        match self.action {
            UpdateAction::DockerRestart => {
                if let Ok(_) = self.restart(docker_client).await {
                    self.current_version = new_version;
                }
            }
            _ => todo!(),
        }
    }

    /// Restart a container
    ///
    /// For containers which have an update command in their entrypoint scripts. Many cimages from
    /// docker hub follow this pattern.
    pub async fn restart(&self, docker_client: &Docker) -> Result<(), Box<dyn std::error::Error>> {
        debug!("Restarting container {}", self.name);
        match docker_client.restart_container(&self.name, None).await {
            Err(e) => {
                error!("FAILED to restart container {}: {}", self.name, &e);
                Err(Box::new(e))
            }
            _ => {
                info!("Container {} successfully updated via restart", self.name);
                Ok(())
            }
        }
    }

    /// Save the state of the container to disk
    ///
    /// Creates/updates a {container name}.json file with a simple serialisation of the container
    /// object in it.
    pub fn save_state(&self, dir: &PathBuf) {
        // Save the current state to the save file directory, currently only used to save version
        // between restarts
        debug!("Saving container {}'s state to disk", self.name);
        let serial = match serde_json::to_string(&self) {
            Ok(s) => s,
            Err(e) => panic!("FAILED to serialise container {}: {}", self.name, e),
        };
        let save_path = self.get_save_path(dir);
        match std::fs::write(&save_path, serial) {
            Err(e) => panic!("FAILED saving container {} state to disk: {}", self.name, e),
            _ => (),
        }
    }

    /// Helper method to create the path string for the save file for this container
    fn get_save_path(&self, dir: &PathBuf) -> PathBuf {
        [dir.to_str().unwrap(), &format!("{}.json", self.name)]
            .iter()
            .collect()
    }
}
