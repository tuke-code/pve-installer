use crate::SummaryOption;

use proxmox_installer_common::{
    options::{BootdiskOptions, NetworkOptions, TimezoneOptions},
    setup::{LocaleInfo, NetworkInfo, RuntimeInfo},
};
use proxmox_installer_types::{EMAIL_DEFAULT_PLACEHOLDER, answer::NetworkBondOptions};

#[derive(Clone)]
pub struct PasswordOptions {
    pub email: String,
    pub root_password: String,
}

impl Default for PasswordOptions {
    fn default() -> Self {
        Self {
            email: EMAIL_DEFAULT_PLACEHOLDER.to_string(),
            root_password: String::new(),
        }
    }
}

#[derive(Clone)]
pub struct InstallerOptions {
    pub bootdisk: BootdiskOptions,
    pub timezone: TimezoneOptions,
    pub password: PasswordOptions,
    pub network: NetworkOptions,
    pub autoreboot: bool,
}

impl InstallerOptions {
    pub fn to_summary(
        &self,
        runtime_info: &RuntimeInfo,
        locales: &LocaleInfo,
    ) -> Vec<SummaryOption> {
        let kb_layout = locales
            .kmap
            .get(&self.timezone.kb_layout)
            .map(|l| &l.name)
            .unwrap_or(&self.timezone.kb_layout);

        let mngmt_interface = match &self.network.bond_opts {
            Some(opts) => format!("bond0 (mode: {})", opts.mode),
            None => self.network.ifname.to_owned(),
        };

        let mut summary = vec![
            SummaryOption::new("Bootdisk filesystem", self.bootdisk.fstype.to_string()),
            SummaryOption::new(
                "Bootdisk(s)",
                self.bootdisk
                    .disks
                    .iter()
                    .map(|d| d.path.as_str())
                    .collect::<Vec<&str>>()
                    .join(", "),
            ),
            SummaryOption::new("Timezone", &self.timezone.timezone),
            SummaryOption::new("Keyboard layout", kb_layout),
            SummaryOption::new("Administrator email", &self.password.email),
            SummaryOption::new("Management interface", &mngmt_interface),
        ];

        if let Some(opts) = &self.network.bond_opts {
            let members = self
                .resolve_bond_member_names(opts, &runtime_info.network)
                .join(" | ");
            summary.push(SummaryOption::new("Bond members", members));
        }

        if let Some(opts) = &self.network.vlan_opts {
            summary.push(SummaryOption::new("VLAN tag", opts.vid.to_string()));
        }

        summary.extend([
            SummaryOption::new("Hostname", self.network.fqdn.to_string()),
            SummaryOption::new("Host IP (CIDR)", self.network.address.to_string()),
            SummaryOption::new("Gateway", self.network.gateway.to_string()),
            SummaryOption::new("DNS", self.network.dns_server.to_string()),
        ]);

        summary
    }

    fn resolve_bond_member_names(
        &self,
        bond: &NetworkBondOptions,
        network: &NetworkInfo,
    ) -> Vec<String> {
        bond.members
            .iter()
            .map(|mac| {
                network.physical_interface_by_mac(mac).map_or_else(
                    || "<unknown>".to_owned(),
                    |iface| iface.installed_name(self.network.pinning_opts.as_ref()),
                )
            })
            .collect()
    }
}
