use anyhow::{Error, Result};
use log::{info, warn};
use serde::Deserialize;
use serde_json;
use std::{
    fs::{self, create_dir_all},
    path::{Path, PathBuf},
    process::Command,
};

static ANSWER_MP: &str = "/mnt/answer";
static PARTLABEL: &str = "proxmoxinst";
static SEARCH_PATH: &str = "/dev/disk/by-label";

pub mod post;
pub mod sysinfo;

/// Searches for upper and lower case existence of the partlabel in the search_path
///
/// # Arguemnts
/// * `partlabel_source` - Partition Label, used as upper and lower case
/// * `search_path` - Path where to search for the partiiton label
pub fn scan_partlabels(partlabel_source: &str, search_path: &str) -> Result<PathBuf> {
    let partlabel = partlabel_source.to_uppercase();
    let path = Path::new(search_path).join(&partlabel);
    match path.try_exists() {
        Ok(true) => {
            info!("Found partition with label '{}'", partlabel);
            return Ok(path);
        }
        Ok(false) => info!("Did not detect partition with label '{}'", partlabel),
        Err(err) => info!("Encountered issue, accessing '{}': {}", path.display(), err),
    }

    let partlabel = partlabel_source.to_lowercase();
    let path = Path::new(search_path).join(&partlabel);
    match path.try_exists() {
        Ok(true) => {
            info!("Found partition with label '{}'", partlabel);
            return Ok(path);
        }
        Ok(false) => info!("Did not detect partition with label '{}'", partlabel),
        Err(err) => info!("Encountered issue, accessing '{}': {}", path.display(), err),
    }
    Err(Error::msg(format!(
        "Could not detect upper or lower case labels for '{partlabel_source}'"
    )))
}

/// Will search and mount a partition/FS labeled proxmoxinst in lower or uppercase to ANSWER_MP;
pub fn mount_proxmoxinst_part() -> Result<String> {
    if let Ok(true) = check_if_mounted(ANSWER_MP) {
        info!("Skipping: '{ANSWER_MP}' is already mounted.");
        return Ok(ANSWER_MP.into());
    }
    let part_path = scan_partlabels(PARTLABEL, SEARCH_PATH)?;
    info!("Mounting partition at {ANSWER_MP}");
    // create dir for mountpoint
    create_dir_all(ANSWER_MP)?;
    match Command::new("mount")
        .args(["-o", "ro"])
        .arg(part_path)
        .arg(ANSWER_MP)
        .output()
    {
        Ok(output) => {
            if output.status.success() {
                Ok(ANSWER_MP.into())
            } else {
                warn!("Error mounting: {}", String::from_utf8(output.stderr)?);
                Ok(ANSWER_MP.into())
            }
        }
        Err(err) => Err(Error::msg(format!("Error mounting: {err}"))),
    }
}

fn check_if_mounted(target_path: &str) -> Result<bool> {
    let mounts = fs::read_to_string("/proc/mounts")?;
    for line in mounts.lines() {
        if let Some(mp) = line.split(' ').nth(1) {
            if mp == target_path {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[derive(Deserialize, Debug)]
struct IpLinksUdevInfo {
    ifname: String,
}

/// Returns vec of usable NICs
pub fn get_nic_list() -> Result<Vec<String>> {
    let ip_output = Command::new("/usr/sbin/ip")
        .arg("-j")
        .arg("link")
        .output()?;
    let parsed_links: Vec<IpLinksUdevInfo> =
        serde_json::from_str(String::from_utf8(ip_output.stdout)?.as_str())?;
    let mut links: Vec<String> = Vec::new();

    for link in parsed_links {
        if link.ifname == *"lo" {
            continue;
        }
        links.push(link.ifname);
    }

    Ok(links)
}
