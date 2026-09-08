use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser::SerializeMap};
use std::{
    cmp,
    collections::{BTreeMap, HashMap},
    fmt,
    fs::File,
    hash::Hash,
    io::{self, BufReader},
    net::IpAddr,
    path::{Path, PathBuf},
    process::{self, Command, Stdio},
};

use crate::options::{
    BtrfsBootdiskOptions, Disk, NetworkInterfacePinningOptions, ZfsBootdiskOptions,
};
use proxmox_installer_types::{
    BootType, IsoInfo, ProductConfig,
    answer::{
        BtrfsCompressOption, FilesystemType, NetworkBondOptions, ZfsChecksumOption,
        ZfsCompressOption,
    },
};
use proxmox_network_types::{
    Cidr, MacAddress,
    bond::{BondXmitHashPolicy, LinuxBondMode},
};

/// Paths in the ISO environment containing installer data.
#[derive(Clone, Deserialize)]
pub struct IsoLocations {
    pub iso: PathBuf,
    pub lib: PathBuf,
}

impl IsoLocations {
    /// A mocked location, uses the current working directory by default
    pub fn mocked() -> Self {
        let lib = match std::env::current_dir() {
            Ok(mut dir) => {
                dir.push("run");
                dir
            }
            Err(_) => "/dev/null".into(),
        };

        Self {
            iso: std::env::current_dir().unwrap_or("/dev/null".into()),
            lib,
        }
    }
}

#[derive(Clone, Deserialize)]
pub struct SetupInfo {
    #[serde(rename = "product-cfg")]
    pub config: ProductConfig,
    #[serde(rename = "iso-info")]
    pub iso_info: IsoInfo,
    pub locations: IsoLocations,
}

impl SetupInfo {
    /// Return a mocked SetupInfo that is very similar to how our actual ones look like and should
    /// be good enough for testing.
    pub fn mocked() -> Self {
        Self {
            config: ProductConfig::mocked(),
            iso_info: IsoInfo::mocked(),
            locations: IsoLocations::mocked(),
        }
    }
}

#[derive(Clone, Deserialize)]
pub struct CountryInfo {
    pub name: String,
    #[serde(default)]
    pub zone: String,
    pub kmap: String,
}

#[derive(Clone, Deserialize, Eq, PartialEq)]
pub struct KeyboardMapping {
    pub name: String,
    #[serde(rename = "kvm")]
    pub id: String,
    #[serde(rename = "x11")]
    pub xkb_layout: String,
    #[serde(rename = "x11var")]
    pub xkb_variant: String,
}

impl cmp::PartialOrd for KeyboardMapping {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl cmp::Ord for KeyboardMapping {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        self.name.cmp(&other.name)
    }
}

#[derive(Clone, Deserialize)]
pub struct LocaleInfo {
    #[serde(deserialize_with = "deserialize_cczones_map")]
    pub cczones: HashMap<String, Vec<String>>,
    #[serde(rename = "country")]
    pub countries: HashMap<String, CountryInfo>,
    pub kmap: HashMap<String, KeyboardMapping>,
}

/// Fetches basic information needed for the installer which is required to work
pub fn installer_setup(in_test_mode: bool) -> Result<(SetupInfo, LocaleInfo, RuntimeInfo), String> {
    let base_path = if in_test_mode {
        format!("./testdir/{}", crate::RUNTIME_DIR)
    } else {
        crate::RUNTIME_DIR.to_owned()
    };

    load_installer_setup_files(base_path)
}

pub fn load_installer_setup_files(
    runtime_dir: impl AsRef<Path>,
) -> Result<(SetupInfo, LocaleInfo, RuntimeInfo), String> {
    let installer_info: SetupInfo = {
        let mut path = runtime_dir.as_ref().to_path_buf();
        path.push("iso-info.json");

        read_json(&path)
            .map_err(|err| format!("Failed to retrieve setup info: {}: {err}", path.display()))?
    };

    let locale_info = {
        let mut path = installer_info.locations.lib.clone();
        path.push("locale-info.json");

        read_json(&path)
            .map_err(|err| format!("Failed to retrieve locale info: {}: {err}", path.display()))?
    };

    let mut runtime_info: RuntimeInfo = {
        let mut path = runtime_dir.as_ref().to_path_buf();
        path.push("run-env-info.json");

        read_json(&path).map_err(|err| {
            format!(
                "Failed to retrieve runtime environment info: {}: {err}",
                path.display()
            )
        })?
    };

    runtime_info.disks.sort();
    if runtime_info.disks.is_empty() {
        Err("The installer could not find any supported hard disks.".to_owned())
    } else if runtime_info.network.interfaces.is_empty() {
        Err("The installer could not find any supported network interface cards.".to_owned())
    } else {
        Ok((installer_info, locale_info, runtime_info))
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct InstallZfsOption {
    pub ashift: usize,
    pub compress: ZfsCompressOption,
    pub checksum: ZfsChecksumOption,
    pub copies: usize,
    pub arc_max: usize,
}

impl From<ZfsBootdiskOptions> for InstallZfsOption {
    fn from(opts: ZfsBootdiskOptions) -> Self {
        InstallZfsOption {
            ashift: opts.ashift,
            compress: opts.compress,
            checksum: opts.checksum,
            copies: opts.copies,
            arc_max: opts.arc_max,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct InstallBtrfsOption {
    #[serde(serialize_with = "serialize_as_display")]
    pub compress: BtrfsCompressOption,
}

impl From<BtrfsBootdiskOptions> for InstallBtrfsOption {
    fn from(opts: BtrfsBootdiskOptions) -> Self {
        InstallBtrfsOption {
            compress: opts.compress,
        }
    }
}

pub fn read_json<T: for<'de> Deserialize<'de>, P: AsRef<Path>>(path: P) -> Result<T, String> {
    let file = File::open(path).map_err(|err| err.to_string())?;
    let reader = BufReader::new(file);

    serde_json::from_reader(reader).map_err(|err| format!("failed to parse JSON: {err}"))
}

fn deserialize_bool_from_int<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    let val: u32 = Deserialize::deserialize(deserializer)?;
    Ok(val != 0)
}

fn deserialize_cczones_map<'de, D>(
    deserializer: D,
) -> Result<HashMap<String, Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    let map: HashMap<String, HashMap<String, u32>> = Deserialize::deserialize(deserializer)?;

    let mut result = HashMap::new();
    for (cc, list) in map.into_iter() {
        result.insert(cc, list.into_keys().collect());
    }

    Ok(result)
}

fn deserialize_disks_map<'de, D>(deserializer: D) -> Result<Vec<Disk>, D::Error>
where
    D: Deserializer<'de>,
{
    let disks =
        <Vec<(usize, String, f64, String, Option<usize>, String)>>::deserialize(deserializer)?;
    Ok(disks
        .into_iter()
        .map(
            |(index, device, size_mb, model, logical_bsize, _syspath)| Disk {
                index: index.to_string(),
                // Linux always reports the size of block devices in sectors, where one sector is
                // defined as being 2^9 = 512 bytes in size.
                // https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/include/linux/blk_types.h?h=v6.4#n30
                size: (size_mb * 512.) / 1024. / 1024. / 1024.,
                block_size: logical_bsize,
                path: device,
                model: (!model.is_empty()).then_some(model),
            },
        )
        .collect())
}

fn deserialize_cidr_list<'de, D>(deserializer: D) -> Result<Vec<Cidr>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    struct CidrDescriptor {
        address: String,
        prefix: u8,
        // family is implied anyway by parsing the address
    }

    let list: Vec<CidrDescriptor> = Deserialize::deserialize(deserializer)?;

    let mut result = Vec::with_capacity(list.len());
    for desc in list {
        let ip_addr = desc
            .address
            .parse::<IpAddr>()
            .map_err(|err| de::Error::custom(format!("{:?}", err)))?;

        result.push(
            Cidr::new(ip_addr, desc.prefix)
                .map_err(|err| de::Error::custom(format!("{:?}", err)))?,
        );
    }

    Ok(result)
}

/// Leaves out links without an Ethernet MAC address, like InfiniBand ones, instead of failing to
/// start the installer for them.
fn deserialize_ethernet_interfaces<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, Interface>, D::Error>
where
    D: Deserializer<'de>,
{
    let interfaces: BTreeMap<String, serde_json::Value> = Deserialize::deserialize(deserializer)?;

    interfaces
        .into_iter()
        .filter(|(_, iface)| {
            iface["mac"]
                .as_str()
                .is_some_and(|mac| mac.parse::<MacAddress>().is_ok())
        })
        .map(|(name, iface)| {
            Interface::deserialize(iface)
                .map(|iface| (name, iface))
                .map_err(de::Error::custom)
        })
        .collect()
}

fn serialize_as_display<S, T>(value: &T, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    T: fmt::Display,
{
    serializer.collect_str(value)
}

fn serialize_bool_as_u32<S>(value: &bool, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_u32(if *value { 1 } else { 0 })
}

#[derive(Clone, Deserialize)]
pub struct RuntimeInfo {
    /// Whether is system was booted in (legacy) BIOS or UEFI mode.
    pub boot_type: BootType,

    /// Detected country if available.
    pub country: Option<String>,

    /// Maps devices to their information.
    #[serde(deserialize_with = "deserialize_disks_map")]
    pub disks: Vec<Disk>,

    /// Network addresses, gateways and DNS info.
    pub network: NetworkInfo,

    /// Total memory of the system in MiB.
    pub total_memory: usize,

    /// Whether the CPU supports hardware-accelerated virtualization
    #[serde(deserialize_with = "deserialize_bool_from_int")]
    pub hvm_supported: bool,

    /// Whether the system was booted with SecureBoot enabled
    #[serde(default, deserialize_with = "deserialize_bool_from_int")]
    pub secure_boot: bool,

    /// Default upper limit for the ZFS ARC size, in MiB.
    pub default_zfs_arc_max: usize,
}

#[derive(Clone, Deserialize)]
pub struct NetworkInfo {
    pub dns: Dns,
    pub routes: Routes,

    /// Maps devices to their configuration, if it has a usable configuration, keyed by their
    /// kernel-assigned interface name.
    /// (Contains no entries for devices with only link-local addresses.)
    #[serde(default, deserialize_with = "deserialize_ethernet_interfaces")]
    pub interfaces: BTreeMap<String, Interface>,

    /// The hostname of this machine, if set by the DHCP server.
    pub hostname: Option<String>,
}

#[derive(Clone, Deserialize)]
pub struct Dns {
    pub domain: Option<String>,

    /// List of stringified IP addresses.
    #[serde(default)]
    pub dns: Vec<IpAddr>,
}

#[derive(Clone, Deserialize)]
pub struct Routes {
    /// Ipv4 gateway.
    pub gateway4: Option<Gateway>,

    /// Ipv6 gateway.
    pub gateway6: Option<Gateway>,
}

#[derive(Clone, Deserialize)]
pub struct Gateway {
    /// Outgoing network device.
    pub dev: String,

    /// Stringified gateway IP address.
    pub gateway: IpAddr,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "UPPERCASE")]
/// The current interface state as reported by the kernel.
pub enum InterfaceState {
    Up,
    Down,
    #[serde(other)]
    Unknown,
}

serde_plain::derive_display_from_serialize!(InterfaceState);

impl InterfaceState {
    // avoid display trait as this is not the string representation for a serializer
    pub fn render(&self) -> String {
        match self {
            Self::Up => "\u{25CF}",
            Self::Down | Self::Unknown => " ",
        }
        .into()
    }
}

#[derive(Clone, Deserialize)]
pub struct Interface {
    pub name: String,

    pub index: usize,

    /// Sequential interface ID for pinning interface names. If unset, the
    /// interface name cannot/should not be pinned due to being e.g. a non-physical link.
    pub pinned_id: Option<String>,

    pub mac: MacAddress,

    pub state: InterfaceState,

    /// Kernel driver name that claims this interface.
    pub driver: String,

    #[serde(default)]
    #[serde(deserialize_with = "deserialize_cidr_list")]
    pub addresses: Vec<Cidr>,
}

impl Interface {
    // avoid display trait as this is not the string representation for a serializer
    pub fn render(&self) -> String {
        format!(
            "{} {} ({:x}, {})",
            self.state.render(),
            self.name,
            self.mac,
            self.driver
        )
    }

    pub fn to_pinned(&self, options: &NetworkInterfacePinningOptions) -> Option<Self> {
        if let Some(pinned_id) = &self.pinned_id {
            let mut this = self.clone();
            this.name = options
                .mapping
                .get(&this.mac)
                .unwrap_or(&format!(
                    "{}{}",
                    NetworkInterfacePinningOptions::DEFAULT_PREFIX,
                    pinned_id
                ))
                .clone();

            Some(this)
        } else {
            None
        }
    }

    /// Returns the name the interface has on the installed system.
    pub fn installed_name(&self, pinning_opts: Option<&NetworkInterfacePinningOptions>) -> String {
        pinning_opts
            .and_then(|opts| self.to_pinned(opts))
            .map_or_else(|| self.name.clone(), |iface| iface.name)
    }
}

impl NetworkInfo {
    /// Returns the physical interface with the given MAC address. Links sharing one cannot be told
    /// apart by it, so the first physical one by name stands for all of them, like for the
    /// low-level installer.
    pub fn physical_interface_by_mac(&self, mac: &MacAddress) -> Option<&Interface> {
        self.interfaces
            .values()
            .find(|iface| iface.pinned_id.is_some() && iface.mac == *mac)
    }
}

/// Root password which can either be in plain-text or hashed format.
///
/// The serialized format is suitable for passing to the low-level installer.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InstallRootPassword {
    Plain(String),
    Hashed(String),
}

impl fmt::Debug for InstallRootPassword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InstallRootPassword::Plain(_) => write!(f, "********"),
            InstallRootPassword::Hashed(s) => write!(f, "{}", s),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct InstallFirstBootSetup {
    #[serde(
        serialize_with = "serialize_bool_as_u32",
        deserialize_with = "deserialize_bool_from_int"
    )]
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ordering_target: Option<String>,
}

pub fn spawn_low_level_installer(test_mode: bool) -> io::Result<process::Child> {
    let (path, args, envs): (&str, &[&str], Vec<(&str, &str)>) = if test_mode {
        (
            "./proxmox-low-level-installer",
            &["-t", "/dev/null", "start-session-test"],
            vec![("PERL5LIB", ".")],
        )
    } else {
        ("proxmox-low-level-installer", &["start-session"], vec![])
    };

    Command::new(path)
        .args(args)
        .envs(envs)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
}

/// See Proxmox::Install::Config
#[derive(Debug, Deserialize, Serialize)]
pub struct InstallConfig {
    pub autoreboot: usize,

    pub filesys: FilesystemType,
    pub hdsize: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swapsize: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maxroot: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minfree: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maxvz: Option<f64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub zfs_opts: Option<InstallZfsOption>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub btrfs_opts: Option<InstallBtrfsOption>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_hd: Option<String>,

    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub disk_selection: BTreeMap<String, String>,

    pub existing_storage_auto_rename: usize,

    pub country: String,
    pub timezone: String,
    pub keymap: String,

    pub root_password: InstallRootPassword,
    pub mailto: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub root_ssh_keys: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription_key: Option<String>,

    pub mngmt_nic: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mngmt_bond: Option<InstallBondConfig>,

    // Maps MAC addresses -> custom name. If set, enables pinning for all
    // interfaces present.
    #[serde(
        default,
        skip_serializing_if = "HashMap::is_empty",
        deserialize_with = "deserialize_optional_map",
        serialize_with = "serialize_low_level_mac_address_map"
    )]
    pub network_interface_pin_map: HashMap<MacAddress, String>,

    pub hostname: String,
    pub domain: String,
    pub cidr: Cidr,
    pub gateway: IpAddr,
    pub dns: IpAddr,

    pub first_boot: InstallFirstBootSetup,
}

/// Management bond setup, see `mngmt_bond` in Proxmox::Install::Config
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct InstallBondConfig {
    #[serde(serialize_with = "serialize_low_level_mac_addresses")]
    pub members: Vec<MacAddress>,
    pub mode: LinuxBondMode,
    pub hash_policy: BondXmitHashPolicy,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_low_level_optional_mac_address"
    )]
    pub primary_interface: Option<MacAddress>,
}

impl From<&NetworkBondOptions> for InstallBondConfig {
    fn from(opts: &NetworkBondOptions) -> Self {
        Self {
            members: opts.members.clone(),
            mode: opts.mode,
            hash_policy: opts.hash_policy,
            primary_interface: opts.primary_interface,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum LowLevelMessage {
    #[serde(rename = "message")]
    Info {
        message: String,
    },
    Error {
        message: String,
    },
    Prompt {
        query: String,
    },
    Finished {
        state: String,
        message: String,
    },
    Progress {
        ratio: f32,
        text: Option<String>,
    },
}

/// Deserializes an optional [HashMap].
///
/// If missing, it returns an empty map, otherwise the deserialized map.
fn deserialize_optional_map<'de, D, K, V>(deserializer: D) -> Result<HashMap<K, V>, D::Error>
where
    D: Deserializer<'de>,
    K: Eq + Hash + Deserialize<'de>,
    V: Deserialize<'de>,
{
    let map: Option<HashMap<K, V>> = Deserialize::deserialize(deserializer)?;
    Ok(map.unwrap_or_default())
}

/// Serializes a [HashMap] using [MacAddress] as keys, normalizing MAC addresses into their
/// lower-case format as the low-level installer expects it.
fn serialize_low_level_mac_address_map<S, V>(
    map: &HashMap<MacAddress, V>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    V: Serialize,
{
    let mut serializer = serializer.serialize_map(Some(map.len()))?;
    for (mac, v) in map {
        serializer.serialize_entry(&format!("{mac:x}"), &v)?;
    }
    serializer.end()
}

/// Serializes a list of [MacAddress]es in their lower-case format as the low-level installer
/// expects it.
fn serialize_low_level_mac_addresses<S>(
    macs: &[MacAddress],
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.collect_seq(macs.iter().map(|mac| format!("{mac:x}")))
}

/// Serializes an optional [MacAddress] in its lower-case format as the low-level installer
/// expects it.
fn serialize_low_level_optional_mac_address<S>(
    mac: &Option<MacAddress>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match mac {
        Some(mac) => serializer.serialize_some(&format!("{mac:x}")),
        None => serializer.serialize_none(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_info_leaves_out_links_without_ethernet_mac_address() {
        let network: NetworkInfo = serde_json::from_str(
            r#"{
                "dns": {},
                "routes": {},
                "interfaces": {
                    "eno1": {
                        "name": "eno1", "index": 2, "pinned_id": "0", "mac": "b4:2e:99:ac:ad:b4",
                        "state": "UP", "driver": "igb"
                    },
                    "ib0": {
                        "name": "ib0", "index": 3, "pinned_id": "1",
                        "mac": "80:00:02:08:fe:80:00:00:00:00:00:00:00:02:c9:03:00:0e:1a:91",
                        "state": "DOWN", "driver": "mlx4_core"
                    }
                }
            }"#,
        )
        .unwrap();

        assert_eq!(network.interfaces.keys().collect::<Vec<_>>(), ["eno1"]);
    }

    #[test]
    fn physical_interface_by_mac_is_first_physical_link_by_name() {
        let network: NetworkInfo = serde_json::from_str(
            r#"{
                "dns": {},
                "routes": {},
                "interfaces": {
                    "br0": {
                        "name": "br0", "index": 2, "mac": "b4:2e:99:ac:ad:b4", "state": "UP",
                        "driver": "bridge"
                    },
                    "enp2s0": {
                        "name": "enp2s0", "index": 4, "pinned_id": "1", "mac": "b4:2e:99:ac:ad:b4",
                        "state": "UP", "driver": "igb"
                    },
                    "enp1s0": {
                        "name": "enp1s0", "index": 3, "pinned_id": "0", "mac": "b4:2e:99:ac:ad:b4",
                        "state": "UP", "driver": "igb"
                    }
                }
            }"#,
        )
        .unwrap();

        let mac = "b4:2e:99:ac:ad:b4".parse().unwrap();
        let iface = network.physical_interface_by_mac(&mac).unwrap();
        assert_eq!(iface.name, "enp1s0");

        let unknown = "b4:2e:99:ac:ad:b5".parse().unwrap();
        assert!(network.physical_interface_by_mac(&unknown).is_none());
    }

    #[test]
    fn bond_config_uses_keys_of_low_level_installer() {
        // the saved low-level configuration keeps this default without a bond
        let disabled: InstallBondConfig = serde_json::from_str(
            r#"{"members":[],"mode":"active-backup","hash_policy":"layer2","primary_interface":null}"#,
        )
        .unwrap();
        assert!(disabled.members.is_empty());
        assert_eq!(disabled.primary_interface, None);

        let bond = InstallBondConfig {
            members: vec![
                "B4:2E:99:AC:AD:B4".parse().unwrap(),
                "b4:2e:99:ac:ad:b5".parse().unwrap(),
            ],
            mode: LinuxBondMode::ActiveBackup,
            hash_policy: BondXmitHashPolicy::Layer2,
            primary_interface: Some("B4:2E:99:AC:AD:B5".parse().unwrap()),
        };
        assert_eq!(
            serde_json::to_string(&bond).unwrap(),
            r#"{"members":["b4:2e:99:ac:ad:b4","b4:2e:99:ac:ad:b5"],"mode":"active-backup","hash_policy":"layer2","primary_interface":"b4:2e:99:ac:ad:b5"}"#
        );
    }
}
