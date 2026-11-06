use anyhow::{Result, anyhow, bail};
use cursive::{
    Cursive, View,
    view::{Finder, Nameable, Resizable, ViewWrapper},
    views::{
        BoxedView, Button, Checkbox, Dialog, DummyView, EditView, LinearLayout, NamedView,
        PaddedView, ResizedView, ScrollView, SelectView, TextView,
    },
};
use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{Arc, Mutex},
};

use proxmox_installer_common::{
    net::MAX_IFNAME_LEN,
    options::{NetworkInterfacePinningOptions, NetworkOptions},
    setup::{Interface, NetworkInfo},
};
use proxmox_installer_types::answer::{NetworkBondOptions, NetworkVlanOptions};
use proxmox_network_types::{
    MacAddress,
    bond::{BondXmitHashPolicy, LinuxBondMode},
    fqdn::Fqdn,
    ip_address::Cidr,
};

use super::{CheckboxGroup, CidrAddressEditView, FormView, IntegerEditView, TabbedView};

struct NetworkViewOptions {
    pinning_enabled: bool,
    bond_enabled: bool,
    vlan_enabled: bool,
    network: NetworkInfo,
    interfaces: Vec<Interface>,
    /// Last selected interface (by its MAC address) in the [SelectView].
    /// Keeps track of the selected interface across enabling/disabling bonding, which replaces the
    /// view.
    last_selected_iface: MacAddress,
    // kept while disabled, so that toggling the checkbox does not reset them
    pinning_options: NetworkInterfacePinningOptions,
    bond_options: NetworkBondOptions,
    vlan_options: NetworkVlanOptions,
}

/// Convenience wrapper when needing to take a (interior-mutable) reference to
/// `NetworkViewOptions`.
type NetworkViewOptionsRef = Arc<Mutex<NetworkViewOptions>>;

/// View for configuring anything related to network setup.
pub struct NetworkOptionsView {
    view: LinearLayout,
    options: NetworkViewOptionsRef,
}

impl NetworkOptionsView {
    const FORM_VIEW_NAME: &str = "network-management-form";
    const MGMT_IFNAME_SELECTVIEW_NAME: &str = "network-management-ifname-selectview";
    const ADVANCED_OPTIONS_BUTTON_NAME: &str = "network-advanced-options-button";

    const INTERFACE_PINNING_VIEW_NAME: &str = "network-name-pinning-view";
    const BOND_OPTIONS_VIEW_NAME: &str = "network-bond-options-view";
    const VLAN_OPTIONS_VIEW_NAME: &str = "network-vlan-options-view";
    const ADVANCED_OPTIONS_TABS_NAME: &str = "network-advanced-options-tabs";

    const INTERFACE_PINNING_TAB: usize = 0;
    const BOND_OPTIONS_TAB: usize = 1;
    const VLAN_OPTIONS_TAB: usize = 2;

    pub fn new(options: &NetworkOptions, network_info: &NetworkInfo) -> Self {
        let mut interfaces = network_info
            .interfaces
            .values()
            .cloned()
            .collect::<Vec<Interface>>();

        // First, sort interfaces by their link state and then name
        interfaces.sort_unstable_by(|a, b| (&a.state, &a.name).cmp(&(&b.state, &b.name)));

        let selected_mac = network_info
            .interfaces
            .get(&options.ifname)
            .map(|iface| iface.mac)
            .unwrap_or_else(|| {
                interfaces
                    .first()
                    .expect("at least one network interface")
                    .mac
            });

        let options_ref = Arc::new(Mutex::new(NetworkViewOptions {
            pinning_enabled: options.pinning_opts.is_some(),
            bond_enabled: options.bond_opts.is_some(),
            vlan_enabled: options.vlan_opts.is_some(),
            network: network_info.clone(),
            interfaces,
            last_selected_iface: selected_mac,
            pinning_options: options.pinning_opts.clone().unwrap_or_default(),
            bond_options: options.bond_opts.clone().unwrap_or_default(),
            vlan_options: options
                .vlan_opts
                .clone()
                .unwrap_or(NetworkVlanOptions { vid: 0 }),
        }));

        let form = FormView::<()>::new()
            .child(
                "Management interface",
                Self::build_mgmt_ifname_view(options_ref.clone()),
            )
            .child(
                "Hostname (FQDN)",
                EditView::new().content(options.fqdn.to_string()),
            )
            .child(
                "IP address (CIDR)",
                CidrAddressEditView::new().content(options.address),
            )
            .child(
                "Gateway address",
                EditView::new().content(options.gateway.to_string()),
            )
            .child(
                "DNS server address",
                EditView::new().content(options.dns_server.to_string()),
            );

        let advanced_options = LinearLayout::horizontal()
            .child(DummyView.full_width())
            .child(
                Button::new("Advanced options", {
                    let options_ref = options_ref.clone();
                    move |siv| {
                        let mut view = Self::advanced_options_view(options_ref.clone());

                        // Pre-compute the child's layout, since it might depend on the size. Without this,
                        // the view will be empty until focused.
                        // The screen size never changes in our case, so this is completely OK.
                        view.layout(siv.screen_size());

                        siv.add_layer(view);
                    }
                })
                .with_name(Self::ADVANCED_OPTIONS_BUTTON_NAME),
            );

        let view = LinearLayout::vertical()
            .child(form.with_name(Self::FORM_VIEW_NAME))
            .child(DummyView.full_width())
            .child(advanced_options);

        Self {
            view,
            options: options_ref,
        }
    }

    pub fn get_values(&mut self) -> Result<NetworkOptions, String> {
        let selected_iface = self
            .view
            .call_on_name(
                Self::MGMT_IFNAME_SELECTVIEW_NAME,
                |view: &mut SelectView<Interface>| view.selection(),
            )
            .flatten();

        let form = self
            .view
            .get_child_mut(0)
            .and_then(|v| v.downcast_mut::<NamedView<FormView>>())
            .ok_or("failed to retrieve network options form")?
            .get_mut();

        let fqdn = form
            .get_value::<EditView, _>(1)
            .ok_or("failed to retrieve host FQDN")?
            .parse::<Fqdn>()
            .map_err(|err| format!("hostname does not look valid:\n\n{err}"))?;

        let address = form
            .get_value::<CidrAddressEditView, _>(2)
            .ok_or("failed to retrieve host address".to_string())
            .and_then(|(ip_addr, mask)| Cidr::new(ip_addr, mask).map_err(|err| err.to_string()))?;

        let gateway = form
            .get_value::<EditView, _>(3)
            .ok_or("failed to retrieve gateway address")?
            .parse::<IpAddr>()
            .map_err(|err| err.to_string())?;

        let dns_server = form
            .get_value::<EditView, _>(4)
            .ok_or("failed to retrieve DNS server address")?
            .parse::<IpAddr>()
            .map_err(|err| err.to_string())?;

        let options = self.options.lock().map_err(|err| err.to_string())?;

        let pinning_opts = options
            .pinning_enabled
            .then_some(options.pinning_options.clone());

        let bond_opts = options.bond_enabled.then_some(options.bond_options.clone());
        let vlan_opts = options.vlan_enabled.then_some(options.vlan_options.clone());

        let ifname = if options.bond_enabled {
            "bond0".to_owned()
        } else {
            let iface = selected_iface
                .map(|iface| (*iface).clone())
                .ok_or("failed to retrieve management interface name")?;

            pinning_opts
                .as_ref()
                .and_then(|opts| iface.to_pinned(opts))
                .unwrap_or(iface)
                .name
        };

        if address.address().is_ipv4() != gateway.is_ipv4() {
            Err("host and gateway IP address version must not differ".to_owned())
        } else if address.address().is_ipv4() != dns_server.is_ipv4() {
            Err("host and DNS IP address version must not differ".to_owned())
        } else if fqdn.to_string().ends_with(".invalid") {
            Err("hostname does not look valid".to_owned())
        } else {
            Ok(NetworkOptions {
                ifname,
                fqdn,
                address,
                gateway,
                dns_server,
                pinning_opts,
                bond_opts,
                vlan_opts,
            })
        }
    }

    fn advanced_options_view(options_ref: NetworkViewOptionsRef) -> impl View {
        fn page(view: impl View) -> PaddedView<ScrollView<impl View>> {
            PaddedView::lrtb(1, 1, 1, 0, ScrollView::new(view))
        }

        let tabs = TabbedView::new()
            .tab(
                "Interface Name Pinning",
                page(
                    InterfacePinningOptionsView::new(options_ref.clone())
                        .with_name(Self::INTERFACE_PINNING_VIEW_NAME),
                ),
            )
            .tab(
                "Management Bond",
                page(
                    BondOptionsView::new(options_ref.clone())
                        .with_name(Self::BOND_OPTIONS_VIEW_NAME),
                ),
            )
            .tab(
                "Management VLAN",
                page(
                    VlanOptionsView::new(options_ref.clone())
                        .with_name(Self::VLAN_OPTIONS_VIEW_NAME),
                ),
            );

        Dialog::around(tabs.with_name(Self::ADVANCED_OPTIONS_TABS_NAME))
            .title("Advanced network options")
            .button("Ok", {
                move |siv| {
                    match Self::get_view_values(siv, options_ref.clone()) {
                        Ok((pin_opts, bond_opts, vlan_opts)) => {
                            siv.pop_layer();
                            let options = &mut options_ref.lock().expect("unpoisoned lock");
                            options.pinning_options = pin_opts;
                            options.bond_options = bond_opts;
                            options.vlan_options = vlan_opts;
                        }
                        Err((tab, err)) => {
                            siv.call_on_name(
                                Self::ADVANCED_OPTIONS_TABS_NAME,
                                |view: &mut TabbedView| view.set_current(tab),
                            );
                            siv.add_layer(Dialog::info(err.to_string()));
                            return;
                        }
                    }

                    Self::refresh_ifname_view(siv, options_ref.clone());
                }
            })
            .max_size((80, 40))
    }

    /// Returns the values of all advanced options, or the error together with the index of the
    /// tab containing the invalid input.
    fn get_view_values(
        siv: &mut Cursive,
        options_ref: NetworkViewOptionsRef,
    ) -> Result<
        (
            NetworkInterfacePinningOptions,
            NetworkBondOptions,
            NetworkVlanOptions,
        ),
        (usize, anyhow::Error),
    > {
        let (pinning_enabled, bond_enabled, vlan_enabled) = {
            let options = options_ref.lock().expect("unpoisoned lock");
            (
                options.pinning_enabled,
                options.bond_enabled,
                options.vlan_enabled,
            )
        };

        // disabled options may be left empty or invalid, they keep their previous values
        let pin_opts = if pinning_enabled {
            siv.call_on_name(
                Self::INTERFACE_PINNING_VIEW_NAME,
                InterfacePinningOptionsView::get_values,
            )
            .unwrap_or_else(|| bail!("failed to retrieve network interface name pinning options"))
            .map_err(|err| (Self::INTERFACE_PINNING_TAB, err))?
        } else {
            options_ref
                .lock()
                .expect("unpoisoned lock")
                .pinning_options
                .clone()
        };

        let bond_opts = if bond_enabled {
            let bond_opts = siv
                .call_on_name(
                    Self::BOND_OPTIONS_VIEW_NAME,
                    |view: &mut BondOptionsView| view.get_values(&pin_opts),
                )
                .unwrap_or_else(|| bail!("failed to retrieve management network bond options"))
                .map_err(|err| (Self::BOND_OPTIONS_TAB, err))?;
            bond_opts
                .verify()
                .map_err(|err| (Self::BOND_OPTIONS_TAB, err))?;
            bond_opts
        } else {
            options_ref
                .lock()
                .expect("unpoisoned lock")
                .bond_options
                .clone()
        };

        let vlan_opts = if vlan_enabled {
            let vlan_opts = siv
                .call_on_name(Self::VLAN_OPTIONS_VIEW_NAME, VlanOptionsView::get_values)
                .unwrap_or_else(|| bail!("failed to retrieve management network VLAN options"))
                .map_err(|err| (Self::VLAN_OPTIONS_TAB, err))?;
            vlan_opts
                .verify()
                .map_err(|err| (Self::VLAN_OPTIONS_TAB, err))?;
            vlan_opts
        } else {
            options_ref
                .lock()
                .expect("unpoisoned lock")
                .vlan_options
                .clone()
        };

        Ok((pin_opts, bond_opts, vlan_opts))
    }

    fn refresh_ifname_view(siv: &mut Cursive, options_ref: NetworkViewOptionsRef) {
        siv.call_on_name(Self::FORM_VIEW_NAME, |form: &mut FormView<()>| {
            form.replace_child(0, Self::build_mgmt_ifname_view(options_ref));
        });
    }

    /// Builds the management interface selection, or the bond summary if bonding is enabled.
    fn build_mgmt_ifname_view(options_ref: NetworkViewOptionsRef) -> BoxedView {
        let options = options_ref.lock().expect("unpoisoned lock");

        if options.bond_enabled {
            // a wrapped value would shift the labels of all following rows in the form
            BoxedView::boxed(
                TextView::new(format!("bond0 (mode: {})", options.bond_options.mode)).no_wrap(),
            )
        } else {
            drop(options);
            BoxedView::boxed(
                Self::build_mgmt_ifname_selectview(options_ref)
                    .with_name(Self::MGMT_IFNAME_SELECTVIEW_NAME),
            )
        }
    }

    fn build_mgmt_ifname_selectview(options_ref: NetworkViewOptionsRef) -> SelectView<Interface> {
        let options = options_ref.lock().expect("unpoisoned lock");

        // Map all interfaces to a list of (human-readable interface name, [Interface]) pairs
        let ifnames = options
            .interfaces
            .iter()
            .map(|iface| {
                if options.pinning_enabled
                    && let Some(pinned) = iface.to_pinned(&options.pinning_options)
                {
                    (pinned.render(), pinned.clone())
                } else {
                    (iface.render(), (*iface).clone())
                }
            })
            .collect::<Vec<(String, Interface)>>();

        let mut view = SelectView::new()
            .popup()
            .with_all(ifnames.clone())
            .on_submit({
                let options_ref = options_ref.clone();
                move |_, iface| {
                    let mut options = options_ref.lock().expect("unpoisoned lock");
                    options.last_selected_iface = iface.mac;
                }
            });

        // Finally, (try to) select the current one
        let selected = view
            .iter()
            .position(|(_label, iface)| iface.mac == options.last_selected_iface)
            .unwrap_or(0); // we sort UP interfaces first, so select the first UP interface
        //
        view.set_selection(selected);

        view
    }
}

impl ViewWrapper for NetworkOptionsView {
    cursive::wrap_impl!(self.view: LinearLayout);
}

struct InterfacePinningOptionsView {
    view: LinearLayout,
}

impl InterfacePinningOptionsView {
    const ENABLED_CHECKBOX_NAME: &str = "network-interface-name-pinning-enabled";
    const FORM_NAME: &str = "network-interface-name-pinning-form";

    fn new(options_ref: NetworkViewOptionsRef) -> Self {
        let options = options_ref.lock().expect("unpoisoned lock");

        // Filter out all non-physical links, as it does not make sense to pin their names
        // in this way.
        // The low-level installer will skip them anyway.
        let interfaces = options
            .interfaces
            .iter()
            .filter(|iface| iface.pinned_id.is_some());

        let mut view = LinearLayout::vertical()
            .child(
                LinearLayout::horizontal()
                    .child(
                        Checkbox::new()
                            .with_checked(options.pinning_enabled)
                            .on_change({
                                let options_ref = options_ref.clone();
                                move |siv, enabled| {
                                    options_ref.lock().expect("unpoisoned lock").pinning_enabled =
                                        enabled;

                                    Self::toggle_all(siv, enabled);
                                    BondOptionsView::refresh_ifnames(siv, options_ref.clone());
                                }
                            })
                            .with_name(Self::ENABLED_CHECKBOX_NAME),
                    )
                    .child(TextView::new(" Pin network interface names").no_wrap()),
            )
            .child(DummyView::new());

        let mut form = FormView::<MacAddress>::new();

        for iface in interfaces {
            let label = format!(
                "{:x} ({}, {}, {})",
                iface.mac, iface.name, iface.driver, iface.state
            );

            let view = LinearLayout::horizontal()
                .child(DummyView.full_width()) // right align below form elements
                .child(
                    EditView::new()
                        .content(
                            iface
                                .to_pinned(&options.pinning_options)
                                .expect("always pinnable interface")
                                .name,
                        )
                        .max_content_width(MAX_IFNAME_LEN)
                        .with_enabled(options.pinning_enabled)
                        .fixed_width(MAX_IFNAME_LEN),
                );

            form.add_child_with_data(&label, view, iface.mac);

            if !iface.addresses.is_empty() {
                for chunk in iface.addresses.chunks(2) {
                    let addrs = chunk
                        .iter()
                        .map(|v| v.to_string())
                        .collect::<Vec<String>>()
                        .join(", ");

                    form.add_child_with_custom_label(&format!("  {addrs}\n"), DummyView);
                }
            }
        }

        form.set_labels_enabled(options.pinning_enabled);
        view.add_child(form.with_name(Self::FORM_NAME));

        Self { view }
    }

    fn toggle_all(siv: &mut Cursive, enabled: bool) {
        siv.call_on_name(Self::FORM_NAME, |v: &mut FormView<MacAddress>| {
            v.set_labels_enabled(enabled);
            v.call_on_childs(&|v: &mut LinearLayout| {
                if let Some(v) = v
                    .get_child_mut(1)
                    .and_then(|v| v.downcast_mut::<ResizedView<EditView>>())
                {
                    v.get_inner_mut().set_enabled(enabled);
                }
            });
        });
    }

    fn get_values(&mut self) -> Result<NetworkInterfacePinningOptions> {
        let form = self
            .view
            .get_child_mut(2)
            .and_then(|v| v.downcast_mut::<NamedView<FormView<MacAddress>>>())
            .ok_or(anyhow!("failed to retrieve interface pinning form"))?
            .get_mut();

        let mut mapping = HashMap::new();

        for i in 0..form.len() {
            let (inner, mac) = match form.get_child_with_data::<LinearLayout>(i) {
                Some(formdata) => formdata,
                None => continue,
            };

            let name = inner
                .get_child(1)
                .and_then(|v| v.downcast_ref::<ResizedView<EditView>>())
                .map(|v| v.get_inner().get_content())
                .ok_or(anyhow!(
                    "failed to retrieve pinning ID for interface {}",
                    mac
                ))?;

            mapping.insert(*mac, (*name).clone());
        }

        let opts = NetworkInterfacePinningOptions { mapping };
        opts.verify()?;

        Ok(opts)
    }
}

impl ViewWrapper for InterfacePinningOptionsView {
    cursive::wrap_impl!(self.view: LinearLayout);
}

struct BondOptionsView {
    view: LinearLayout,
    options_ref: NetworkViewOptionsRef,
}

impl BondOptionsView {
    const FORM_NAME: &str = "network-bond-form-view";
    const MEMBERS_VIEW_NAME: &str = "network-bond-members-checkboxes-view";

    fn new(options_ref: NetworkViewOptionsRef) -> Self {
        let options = &options_ref.lock().expect("unpoisoned lock");

        let mut view = LinearLayout::vertical()
            .child(
                LinearLayout::horizontal()
                    .child(
                        Checkbox::new()
                            .with_checked(options.bond_enabled)
                            .on_change({
                                let options_ref = options_ref.clone();
                                move |siv, enabled| {
                                    options_ref.lock().expect("unpoisoned lock").bond_enabled =
                                        enabled;

                                    Self::toggle_all(siv, enabled);
                                }
                            }),
                    )
                    .child(TextView::new(" Enable bonding for the management interface").no_wrap()),
            )
            .child(DummyView::new());

        let mut form = FormView::<()>::new()
            .child(
                "Bond mode",
                SelectView::new()
                    .popup()
                    .with_all(
                        NetworkBondOptions::MODES
                            .iter()
                            .map(|m| (Self::mode_label(*m), *m)),
                    )
                    .selected(
                        NetworkBondOptions::MODES
                            .iter()
                            .position(|m| *m == options.bond_options.mode)
                            .unwrap_or_default(),
                    )
                    .on_submit(Self::toggle_mode_dependent_options),
            )
            .child(
                "Hash policy",
                SelectView::new()
                    .popup()
                    .with_all(
                        NetworkBondOptions::HASH_POLICIES
                            .iter()
                            .map(|m| (m.to_string(), *m)),
                    )
                    .selected(
                        NetworkBondOptions::HASH_POLICIES
                            .iter()
                            .position(|m| *m == options.bond_options.hash_policy)
                            .unwrap_or_default(),
                    ),
            )
            .child(
                "Primary interface",
                EditView::new().content(
                    options
                        .bond_options
                        .primary_interface
                        .and_then(|mac| options.network.physical_interface_by_mac(&mac))
                        .map(|iface| Self::interface_name(options, iface))
                        .unwrap_or_default(),
                ),
            );
        Self::set_options_enabled(&mut form, options.bond_enabled);

        view.add_child(form.with_name(Self::FORM_NAME));
        view.add_child(DummyView::new());

        let mut checkboxes = CheckboxGroup::<MacAddress>::new();
        let is_listed = |iface: &&Interface| {
            options
                .network
                .physical_interface_by_mac(&iface.mac)
                .is_some_and(|member| member.name == iface.name)
        };
        for iface in options.interfaces.iter().filter(is_listed) {
            let label = Self::format_interface_label(options, iface);
            let is_member = options.bond_options.members.contains(&iface.mac);

            checkboxes.add(&label, is_member, iface.mac);
        }
        checkboxes.set_enabled(options.bond_enabled);
        view.add_child(checkboxes.with_name(Self::MEMBERS_VIEW_NAME));

        Self {
            view,
            options_ref: options_ref.clone(),
        }
    }

    fn mode_label(mode: LinuxBondMode) -> String {
        match mode {
            LinuxBondMode::Ieee802_3ad => "LACP (802.3ad)".to_owned(),
            mode => mode.to_string(),
        }
    }

    fn hash_policy_view_enabled(mode: LinuxBondMode) -> bool {
        matches!(mode, LinuxBondMode::Ieee802_3ad | LinuxBondMode::BalanceXor)
    }

    fn primary_interface_view_enabled(mode: LinuxBondMode) -> bool {
        mode == LinuxBondMode::ActiveBackup
    }

    /// Enables or disables the bond options, except those the selected mode does not use.
    fn set_options_enabled(form: &mut FormView, enabled: bool) {
        let mode = form.get_value::<SelectView<LinuxBondMode>, _>(0);
        let hash_policy = enabled && mode.is_some_and(Self::hash_policy_view_enabled);
        let primary = enabled && mode.is_some_and(Self::primary_interface_view_enabled);

        if let Some(v) = form.get_child_mut::<SelectView<LinuxBondMode>>(0) {
            v.set_enabled(enabled);
        }
        if let Some(v) = form.get_child_mut::<SelectView<BondXmitHashPolicy>>(1) {
            v.set_enabled(hash_policy);
        }
        if let Some(v) = form.get_child_mut::<EditView>(2) {
            v.set_enabled(primary);
        }

        form.set_label_enabled(0, enabled);
        form.set_label_enabled(1, hash_policy);
        form.set_label_enabled(2, primary);
    }

    fn toggle_all(siv: &mut Cursive, enabled: bool) {
        siv.call_on_name(Self::FORM_NAME, |form: &mut FormView| {
            Self::set_options_enabled(form, enabled)
        });

        siv.call_on_name(
            Self::MEMBERS_VIEW_NAME,
            |group: &mut CheckboxGroup<MacAddress>| {
                group.set_enabled(enabled);
            },
        );
    }

    fn toggle_mode_dependent_options(siv: &mut Cursive, _: &LinuxBondMode) {
        siv.call_on_name(Self::FORM_NAME, |form: &mut FormView| {
            Self::set_options_enabled(form, true)
        });
    }

    fn refresh_ifnames(siv: &mut Cursive, options_ref: NetworkViewOptionsRef) {
        siv.call_on_name(
            Self::MEMBERS_VIEW_NAME,
            |group: &mut CheckboxGroup<MacAddress>| {
                let options = options_ref.lock().expect("unpoisoned lock");
                group.update_labels(|mac| {
                    options.network.physical_interface_by_mac(mac).map_or_else(
                        || mac.to_string(),
                        |iface| Self::format_interface_label(&options, iface),
                    )
                });
            },
        );
    }

    /// Takes the pinning options as currently entered in the dialog, to resolve the name of the
    /// primary interface.
    fn get_values(
        &mut self,
        pin_opts: &NetworkInterfacePinningOptions,
    ) -> Result<NetworkBondOptions> {
        let form = self
            .view
            .get_child_mut(2)
            .and_then(|v| v.downcast_mut::<NamedView<FormView<()>>>())
            .ok_or(anyhow!("failed to retrieve bond options form"))?
            .get_mut();

        let mode = form
            .get_value::<SelectView<LinuxBondMode>, _>(0)
            .ok_or(anyhow!("failed to retrieve bond mode"))?;

        let hash_policy = form
            .get_value::<SelectView<BondXmitHashPolicy>, _>(1)
            .ok_or(anyhow!("failed to retrieve bond hash policy"))?;

        let primary_name = form
            .get_value::<EditView, String>(2)
            .ok_or(anyhow!("failed to retrieve bond primary interface"))?;

        let members: Vec<MacAddress> = self
            .view
            .get_child_mut(4)
            .and_then(|v| v.downcast_mut::<NamedView<CheckboxGroup<MacAddress>>>())
            .ok_or(anyhow!("failed to retrieve bond member interface group"))?
            .get_mut()
            .entries()
            .filter_map(|(checked, mac)| checked.then_some(*mac))
            .collect();

        if members.len() < 2 {
            bail!("at least 2 member interfaces are required for setting up a bond");
        }

        let primary_interface =
            if Self::primary_interface_view_enabled(mode) && !primary_name.is_empty() {
                let mac = self.get_mac_address_for_interface_name(&primary_name, pin_opts)?;
                if !members.contains(&mac) {
                    bail!("primary interface '{primary_name}' is not a member of the bond");
                }
                Some(mac)
            } else {
                None
            };

        Ok(NetworkBondOptions {
            members,
            mode,
            hash_policy,
            primary_interface,
            ..Default::default()
        })
    }

    /// Name the interface will have on the installed system.
    fn interface_name(options: &NetworkViewOptions, iface: &Interface) -> String {
        iface.installed_name(options.pinning_enabled.then_some(&options.pinning_options))
    }

    fn format_interface_label(options: &NetworkViewOptions, iface: &Interface) -> String {
        let name = Self::interface_name(options, iface);
        let mut label = format!(
            "{name} ({:x}, {}, {})",
            iface.mac, iface.driver, iface.state
        );

        if !iface.addresses.is_empty() {
            label += "\n  ";
            label += iface
                .addresses
                .iter()
                .fold(String::new(), |acc, addr| format!("{acc}, {addr}"))
                .trim_start_matches(", ");
        }

        label
    }

    fn get_mac_address_for_interface_name(
        &self,
        ifname: &str,
        pin_opts: &NetworkInterfacePinningOptions,
    ) -> Result<MacAddress> {
        let options = &self.options_ref.lock().expect("unpoisoned lock");

        if options.pinning_enabled {
            pin_opts
                .mapping
                .iter()
                .find_map(|(mac, name)| (*name == ifname).then_some(*mac))
        } else {
            options
                .interfaces
                .iter()
                .find_map(|iface| (iface.name == ifname).then_some(iface.mac))
        }
        .ok_or_else(|| anyhow!("unknown primary interface '{ifname}'"))
    }
}

impl ViewWrapper for BondOptionsView {
    cursive::wrap_impl!(self.view: LinearLayout);
}

struct VlanOptionsView {
    view: LinearLayout,
}

impl VlanOptionsView {
    const FORM_NAME: &str = "network-vlan-form-view";

    fn new(options_ref: NetworkViewOptionsRef) -> Self {
        let options = &options_ref.lock().expect("unpoisoned lock");

        let mut form = FormView::<()>::new().child("VLAN tag", {
            let view = IntegerEditView::new()
                .allow_empty(true)
                .min_value(1)
                .max_value(4094)
                .enabled(options.vlan_enabled);
            match options.vlan_options.vid {
                0 => view,
                vid => view.content(vid as usize),
            }
        });
        form.set_labels_enabled(options.vlan_enabled);

        let view = LinearLayout::vertical()
            .child(
                LinearLayout::horizontal()
                    .child(
                        Checkbox::new()
                            .with_checked(options.vlan_enabled)
                            .on_change({
                                let options_ref = options_ref.clone();
                                move |siv, enabled| {
                                    options_ref.lock().expect("unpoisoned lock").vlan_enabled =
                                        enabled;
                                    Self::toggle_all(siv, enabled);
                                }
                            }),
                    )
                    .child(
                        TextView::new(" Enable VLAN setup on the management interface").no_wrap(),
                    ),
            )
            .child(DummyView::new())
            .child(form.with_name(Self::FORM_NAME));

        Self { view }
    }

    fn toggle_all(siv: &mut Cursive, enabled: bool) {
        siv.call_on_name(Self::FORM_NAME, |form: &mut FormView| {
            form.set_labels_enabled(enabled);
            if let Some(v) = form.get_child_mut::<IntegerEditView>(0) {
                v.set_enabled(enabled);
            }
        });
    }

    fn get_values(&mut self) -> Result<NetworkVlanOptions> {
        let form = self
            .view
            .get_child_mut(2)
            .and_then(|v| v.downcast_mut::<NamedView<FormView<()>>>())
            .ok_or(anyhow!("failed to retrieve VLAN options form"))?
            .get_mut();

        let vid = form
            .get_child::<IntegerEditView>(0)
            .and_then(|view| view.get_content_maybe())
            .ok_or(anyhow!("VLAN tag must be set"))??;

        Ok(NetworkVlanOptions {
            vid: vid.try_into()?,
        })
    }
}

impl ViewWrapper for VlanOptionsView {
    cursive::wrap_impl!(self.view: LinearLayout);
}
