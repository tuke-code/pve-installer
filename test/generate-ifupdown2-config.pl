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
        "$msg (bridge:$current_cfg->{bridge})",
    );
}

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
                    },
                    eno2 => {
                        mac => 'ab:cd:ef:12:34:56',
                        index => 2,
                        name => 'eno2',
                    },
                },
            },
        };
    },
);
set_config(bridge => 0);
Proxmox::Install::Config::merge({
    mngmt_nic => 'eno1',
    cidr => '192.168.0.10/24',
    gateway => '192.168.0.1',
});

###
# There's essentially a matrix to test w.r.t the overall networking setup:
#
# +-------+--------+
# | test# | bridge |
# +=======+========+
# |   1   |    0   |
# |   2   |    1   |
# +-------+--------+
###

# 1.
set_config(bridge => 0);
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
set_config(bridge => 1);
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

# interface name pinning test
Proxmox::Install::Config::merge({
    network_interface_pin_map => {
        '12:34:56:ab:cd:ef' => 'nic0',
        'ab:cd:ef:12:34:56' => 'nic1',
    },
});
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
                    },
                    eno2 => {
                        mac => 'ab:cd:ef:12:34:56',
                        index => 2,
                        name => 'eno2',
                    },
                },
                routes => {
                    gateways6 => [{ dev => 'eno1', gateway => 'fe80::1', protocol => 'ra' }],
                },
            },
        };
    },
);

set_config(bridge => 0);
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
set_config(bridge => 1);
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

done_testing();
