#!/usr/bin/perl

use strict;
use warnings;

use File::Temp;
use Test::More;
use Test::MockModule qw(strict);

use Proxmox::Install;
use Proxmox::Install::Config;

use Proxmox::Log;
my $log_file = File::Temp->new();
Proxmox::Log::init($log_file->filename);

my $proxmox_install_runenv = Test::MockModule->new('Proxmox::Install::RunEnv');
my $proxmox_install_isoenv = Test::MockModule->new('Proxmox::Install::ISOEnv');

my $current_cfg;
my sub set_config {
    my (%cfg) = @_;

    $current_cfg = \%cfg;
    $proxmox_install_isoenv->redefine(
        get => sub {
            return {
                product => $cfg{bridge} ? 'pve' : 'pbs',
                cfg => {
                    product => $cfg{bridge} ? 'pve' : 'pbs',
                    bridged_network => $cfg{bridge},
                },
            };
        },
    );

    Proxmox::Install::Config::merge({
        mngmt_nic => $cfg{bond} ? 'bond0' : 'eno1',
        mngmt_bond => {
            members => $cfg{bond} ? ['12:34:56:ab:cd:ef', 'ab:cd:ef:12:34:56'] : [],
            mode => 'active-backup',
            hash_policy => 'layer2',
        },
    });
}

my sub is_generated {
    my ($expected, $msg) = @_;

    is(
        Proxmox::Install::generate_ifupdown2_config(), <<EOT,
auto lo
iface lo inet loopback

$expected

source /etc/network/interfaces.d/*
EOT
        "$msg (bridge:$current_cfg->{bridge} bond:$current_cfg->{bond})",
    );
}

my $pin_map = {
    '12:34:56:ab:cd:ef' => 'nic0',
    'ab:cd:ef:12:34:56' => 'nic1',
};

# global config
$proxmox_install_runenv->redefine(
    get => sub {
        return {
            network => {
                interfaces => {
                    eno1 => {
                        mac => '12:34:56:ab:cd:ef',
                        index => 1,
                        name => 'eno1',
                        pinned_id => '0',
                    },
                    eno2 => {
                        mac => 'ab:cd:ef:12:34:56',
                        index => 2,
                        name => 'eno2',
                        pinned_id => '1',
                    },
                },
            },
        };
    },
);
set_config(bridge => 0, bond => 0);
Proxmox::Install::Config::merge({
    cidr => '192.168.0.10/24',
    gateway => '192.168.0.1',
});

###
# There's essentially a matrix to test w.r.t the overall networking setup:
#
# +-------+--------+------+
# | test# | bridge | bond |
# +=======+========+======+
# |   1   |    0   |  0   |
# |   2   |    0   |  1   |
# |   3   |    1   |  0   |
# |   4   |    1   |  1   |
# +-------+--------+------+
###

# 1.
set_config(bridge => 0, bond => 0);
is_generated(
    <<EOT
auto eno1
iface eno1 inet static
	address 192.168.0.10/24
	gateway 192.168.0.1

iface eno2 inet manual
EOT
    , 'host network setup',
);

# 2.
set_config(bridge => 0, bond => 1);
is_generated(
    <<EOT
auto bond0
iface bond0 inet static
	address 192.168.0.10/24
	gateway 192.168.0.1
	bond-slaves eno1 eno2
	bond-miimon 100
	bond-mode active-backup

auto eno1
iface eno1 inet manual

auto eno2
iface eno2 inet manual
EOT
    , 'host network setup',
);

# 3.
set_config(bridge => 1, bond => 0);
is_generated(
    <<EOT
iface eno1 inet manual

auto vmbr0
iface vmbr0 inet static
	address 192.168.0.10/24
	gateway 192.168.0.1
	bridge-ports eno1
	bridge-stp off
	bridge-fd 0

iface eno2 inet manual
EOT
    , 'host network setup',
);

# 4.
set_config(bridge => 1, bond => 1);
is_generated(
    <<EOT
auto bond0
iface bond0 inet manual
	bond-slaves eno1 eno2
	bond-miimon 100
	bond-mode active-backup

auto vmbr0
iface vmbr0 inet static
	address 192.168.0.10/24
	gateway 192.168.0.1
	bridge-ports bond0
	bridge-stp off
	bridge-fd 0

auto eno1
iface eno1 inet manual

auto eno2
iface eno2 inet manual
EOT
    , 'host network setup',
);

# interface name pinning test
Proxmox::Install::Config::merge({
    mngmt_nic => 'eno1',
    network_interface_pin_map => $pin_map,
});
set_config(bridge => 1, bond => 0);
is_generated(
    <<EOT
iface nic0 inet manual

auto vmbr0
iface vmbr0 inet static
	address 192.168.0.10/24
	gateway 192.168.0.1
	bridge-ports nic0
	bridge-stp off
	bridge-fd 0

iface nic1 inet manual
EOT
    , 'management bridge uses pinned interface name',
);
Proxmox::Install::Config::merge({
    network_interface_pin_map => undef,
});

# A gateway learned via IPv6 router advertisements is not written out
Proxmox::Install::Config::merge({
    mngmt_nic => 'eno1',
    cidr => '2001:db8::1/64',
    gateway => 'fe80::1',
});
$proxmox_install_runenv->redefine(
    get => sub {
        return {
            network => {
                interfaces => {
                    eno1 => {
                        mac => '12:34:56:ab:cd:ef',
                        index => 1,
                        name => 'eno1',
                        pinned_id => '0',
                    },
                    eno2 => {
                        mac => 'ab:cd:ef:12:34:56',
                        index => 2,
                        name => 'eno2',
                        pinned_id => '1',
                    },
                },
                routes => {
                    gateways6 => [{ dev => 'eno1', gateway => 'fe80::1', protocol => 'ra' }],
                },
            },
        };
    },
);
set_config(bridge => 0, bond => 0);

is_generated(
    <<EOT
auto eno1
iface eno1 inet6 static
	address 2001:db8::1/64

iface eno2 inet6 manual
EOT
    , 'gateway learned from IPv6 router advertisements is not configured statically',
);

# A bridge does not accept router advertisements, as it forwards
set_config(bridge => 1, bond => 0);
is_generated(
    <<EOT
iface eno1 inet6 manual

auto vmbr0
iface vmbr0 inet6 static
	address 2001:db8::1/64
	gateway fe80::1
	bridge-ports eno1
	bridge-stp off
	bridge-fd 0

iface eno2 inet6 manual
EOT
    , 'gateway learned from IPv6 router advertisements is kept for a bridge',
);

# A gateway learned via IPv6 router advertisements is not written out for a bond
Proxmox::Install::Config::merge({
    mngmt_nic => 'bond0',
    cidr => '2001:db8::1/64',
    gateway => 'fe80::1',
});
set_config(bridge => 0, bond => 1);

is_generated(
    <<EOT
auto bond0
iface bond0 inet6 static
	address 2001:db8::1/64
	bond-slaves eno1 eno2
	bond-miimon 100
	bond-mode active-backup

auto eno1
iface eno1 inet6 manual

auto eno2
iface eno2 inet6 manual
EOT
    , 'gateway learned from IPv6 router advertisements on a bond member is not configured statically',
);

# Check that unpinnable interfaces (e.g. virtual ones) keep their kernel-assigned name
Proxmox::Install::Config::merge({
    network_interface_pin_map => $pin_map,
    cidr => '192.168.0.10/24',
    gateway => '192.168.0.1',
});
set_config(bridge => 0, bond => 0);
$proxmox_install_runenv->redefine(
    get => sub {
        return {
            network => {
                interfaces => {
                    eno1 => {
                        mac => '12:34:56:ab:cd:ef',
                        index => 1,
                        name => 'eno1',
                        pinned_id => '0',
                    },
                    eno2 => {
                        mac => 'ab:cd:ef:12:34:56',
                        index => 2,
                        name => 'eno2',
                        pinned_id => '1',
                    },
                    dummy => {
                        mac => 'aa:bb:cc:dd:ee:ff',
                        index => 3,
                        name => 'dummy',
                    },
                },
            },
        };
    },
);

is_generated(
    <<EOT
auto nic0
iface nic0 inet static
	address 192.168.0.10/24
	gateway 192.168.0.1

iface dummy inet manual

iface nic1 inet manual
EOT
    , 'unpinnable names keep their kernel name',
);

set_config(bridge => 1, bond => 1);
Proxmox::Install::Config::merge({
    mngmt_nic => 'bond0',
    mngmt_bond => {
        members => ['12:34:56:ab:cd:ef', 'ab:cd:ef:12:34:56'],
        mode => 'active-backup',
        hash_policy => 'layer2',
        primary_interface => 'ab:cd:ef:12:34:56',
    },
});
is_generated(
    <<EOT
auto bond0
iface bond0 inet manual
	bond-slaves nic0 nic1
	bond-miimon 100
	bond-mode active-backup
	bond-primary nic1

auto vmbr0
iface vmbr0 inet static
	address 192.168.0.10/24
	gateway 192.168.0.1
	bridge-ports bond0
	bridge-stp off
	bridge-fd 0

iface dummy inet manual

auto nic0
iface nic0 inet manual

auto nic1
iface nic1 inet manual
EOT
    , 'active-backup bond with interface pinning enabled and primary interface set',
);

Proxmox::Install::Config::merge({
    network_interface_pin_map => undef,
    mngmt_bond => {
        members => ['ab:cd:ef:12:34:56', '12:34:56:ab:cd:ef'],
        mode => '802.3ad',
        hash_policy => 'layer3+4',
        # will be ignored on 802.3ad
        primary_interface => 'ab:cd:ef:12:34:56',
    },
});
is_generated(
    <<EOT
auto bond0
iface bond0 inet manual
	bond-slaves eno1 eno2
	bond-miimon 100
	bond-mode 802.3ad
	bond-xmit-hash-policy layer3+4

auto vmbr0
iface vmbr0 inet static
	address 192.168.0.10/24
	gateway 192.168.0.1
	bridge-ports bond0
	bridge-stp off
	bridge-fd 0

iface dummy inet manual

auto eno1
iface eno1 inet manual

auto eno2
iface eno2 inet manual
EOT
    , 'LACP bond enabled, with sorted members, hash policy and no primary interface',
);

Proxmox::Install::Config::merge({
    mngmt_bond => {
        members => ['12:34:56:ab:cd:ff', 'ab:cd:ef:12:34:56'],
        mode => 'active-backup',
        hash_policy => 'layer2',
        primary_interface => undef,
    },
});
eval { is_generated(); };
is(
    $@,
    "no network interface with MAC address '12:34:56:ab:cd:ff' found for the bond\n",
    'unknown bond member is an error',
);

set_config(bridge => 0, bond => 1);
Proxmox::Install::Config::merge({
    network_interface_pin_map => $pin_map,
    mngmt_bond => {
        members => ['12:34:56:AB:CD:EF', 'ab:cd:ef:12:34:56'],
        mode => 'active-backup',
        hash_policy => 'layer2',
        primary_interface => 'AB:cd:ef:12:34:56',
    },
});
is_generated(
    <<EOT
auto bond0
iface bond0 inet static
	address 192.168.0.10/24
	gateway 192.168.0.1
	bond-slaves nic0 nic1
	bond-miimon 100
	bond-mode active-backup
	bond-primary nic1

iface dummy inet manual

auto nic0
iface nic0 inet manual

auto nic1
iface nic1 inet manual
EOT
    , 'bond members and primary interface given with uppercase MAC addresses',
);

# Virtual links like bridges can share the MAC address of a physical one
$proxmox_install_runenv->redefine(
    get => sub {
        return {
            network => {
                interfaces => {
                    br0 => {
                        mac => '12:34:56:ab:cd:ef',
                        index => 4,
                        name => 'br0',
                    },
                    eno1 => {
                        mac => '12:34:56:ab:cd:ef',
                        index => 1,
                        name => 'eno1',
                        pinned_id => '0',
                    },
                    eno2 => {
                        mac => 'ab:cd:ef:12:34:56',
                        index => 2,
                        name => 'eno2',
                        pinned_id => '1',
                    },
                    enp9s0 => {
                        mac => '12:34:56:ab:cd:ef',
                        index => 3,
                        name => 'enp9s0',
                        pinned_id => '2',
                    },
                },
            },
        };
    },
);
Proxmox::Install::Config::merge({
    network_interface_pin_map => undef,
    mngmt_bond => {
        members => ['12:34:56:ab:cd:ef', 'ab:cd:ef:12:34:56'],
        mode => 'active-backup',
        hash_policy => 'layer2',
        primary_interface => undef,
    },
});
like(
    Proxmox::Install::generate_ifupdown2_config(),
    qr/^\tbond-slaves eno1 eno2$/m,
    'bond member sharing its MAC address is the first physical link by name',
);

done_testing();
