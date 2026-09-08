use anyhow::{Result, anyhow, bail};
use cursive::{
    Cursive, View,
    view::{Nameable, Resizable, ViewWrapper},
    views::{
        Button, Checkbox, Dialog, DummyView, EditView, LinearLayout, NamedView, Panel, ResizedView,
        ScrollView, SelectView, TextView,
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
use proxmox_network_types::{MacAddress, fqdn::Fqdn, ip_address::Cidr};

use super::{CidrAddressEditView, FormView};

struct NetworkViewOptions {
    pinning_enabled: bool,
    interfaces: Vec<Interface>,
    // kept while disabled, so that toggling the checkbox does not reset them
    pinning_options: NetworkInterfacePinningOptions,
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
            .map(|iface| iface.mac);

        let options_ref = Arc::new(Mutex::new(NetworkViewOptions {
            pinning_enabled: options.pinning_opts.is_some(),
            interfaces,
            pinning_options: options.pinning_opts.clone().unwrap_or_default(),
        }));

        let iface_selection = Self::build_mgmt_ifname_selectview(options_ref.clone(), selected_mac);

        let form = FormView::<()>::new()
            .child(
                "Management interface",
                iface_selection.with_name(Self::MGMT_IFNAME_SELECTVIEW_NAME),
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
        let form = self
            .view
            .get_child_mut(0)
            .and_then(|v| v.downcast_mut::<NamedView<FormView>>())
            .ok_or("failed to retrieve network options form")?
            .get_mut();

        let iface = form
            .get_value::<NamedView<SelectView<Interface>>, _>(0)
            .ok_or("failed to retrieve management interface name")?;

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

        let pinning_opts = self
            .options
            .lock()
            .map(|opt| opt.pinning_enabled.then_some(opt.pinning_options.clone()))
            .map_err(|err| err.to_string())?;

        let ifname = if let Some(opts) = &pinning_opts
            && let Some(pinned) = iface.to_pinned(opts)
        {
            pinned.name
        } else {
            iface.name
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
                bond_opts: None,
                vlan_opts: None,
            })
        }
    }

    fn advanced_options_view(options_ref: NetworkViewOptionsRef) -> impl View {
        let inner = ScrollView::new(
            LinearLayout::vertical().child(Panel::new(
                InterfacePinningOptionsView::new(options_ref.clone())
                    .with_name(Self::INTERFACE_PINNING_VIEW_NAME),
            )),
        );

        Dialog::around(inner)
            .title("Advanced network options")
            .button("Ok", {
                move |siv| {
                    match Self::get_view_values(siv, options_ref.clone()) {
                        Ok(pin_opts) => {
                            siv.pop_layer();
                            let options = &mut options_ref.lock().expect("unpoisoned lock");
                            options.pinning_options = pin_opts;
                        }
                        Err(err) => {
                            siv.add_layer(Dialog::info(err.to_string()));
                            return;
                        }
                    }

                    Self::refresh_ifname_selectview(siv, options_ref.clone());
                }
            })
            .max_size((80, 40))
    }

    fn get_view_values(
        siv: &mut Cursive,
        options_ref: NetworkViewOptionsRef,
    ) -> Result<NetworkInterfacePinningOptions> {
        let pinning_enabled = options_ref.lock().expect("unpoisoned lock").pinning_enabled;

        // disabled options may be left empty or invalid, they keep their previous values
        let pin_opts = if pinning_enabled {
            siv.call_on_name(
                Self::INTERFACE_PINNING_VIEW_NAME,
                InterfacePinningOptionsView::get_values,
            )
            .unwrap_or_else(|| bail!("failed to retrieve network interface name pinning options"))?
        } else {
            options_ref
                .lock()
                .expect("unpoisoned lock")
                .pinning_options
                .clone()
        };

        Ok(pin_opts)
    }

    fn refresh_ifname_selectview(siv: &mut Cursive, options_ref: NetworkViewOptionsRef) {
        siv.call_on_name(
            Self::MGMT_IFNAME_SELECTVIEW_NAME,
            |view: &mut SelectView<Interface>| {
                let selected_mac = view.selection().map(|iface| iface.mac);
                *view = Self::build_mgmt_ifname_selectview(options_ref, selected_mac);
            },
        );
    }

    fn build_mgmt_ifname_selectview(
        options_ref: NetworkViewOptionsRef,
        selected_mac: Option<MacAddress>,
    ) -> SelectView<Interface> {
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

        let mut view = SelectView::new().popup().with_all(ifnames.clone());

        let selected_mac = selected_mac.unwrap_or_else(|| {
            options
                .interfaces
                .first()
                .expect("at least one network interface")
                .mac
        });

        // Finally, (try to) select the current one
        let selected = view
            .iter()
            .position(|(_label, iface)| iface.mac == selected_mac)
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

        view.add_child(form.with_name(Self::FORM_NAME));

        Self { view }
    }

    fn toggle_all(siv: &mut Cursive, enabled: bool) {
        siv.call_on_name(Self::FORM_NAME, |v: &mut FormView<MacAddress>| {
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
