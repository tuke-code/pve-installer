#!/usr/bin/perl

use strict;
use warnings;

use Test::More;

use Proxmox::Sys::Net;

my $address = { family => 'inet', address => '192.0.2.10', prefix => 24 };

my $network = {
    interfaces => {
        enp1s0 =>
            { name => 'enp1s0', mac => 'aa:bb:cc:dd:ee:01', state => 'UP', pinned_id => '0' },
        enp2s0 =>
            { name => 'enp2s0', mac => 'aa:bb:cc:dd:ee:02', state => 'UP', pinned_id => '1' },
        'live2.10' => {
            name => 'live2.10',
            mac => 'aa:bb:cc:dd:ee:01',
            state => 'UP',
            addresses => [$address],
        },
        'live3.10' => { name => 'live3.10', mac => 'aa:bb:cc:dd:ee:02', state => 'UP' },
    },
    routes => {
        gateway4 => { dev => 'live2.10', gateway => '192.0.2.1', protocol => 'dhcp' },
        gateways6 => [{ dev => 'live3.10', gateway => 'fe80::1', protocol => 'ra' }],
    },
};
$network->{routes}->{gateway6} = $network->{routes}->{gateways6}->[0];

Proxmox::Sys::Net::fold_live_vlan_links(
    $network,
    {
        'live2.10' => { parent => 'enp1s0', vlan_id => 10 },
        'live3.10' => { parent => 'enp2s0', vlan_id => 10 },
    },
);

is_deeply([sort keys $network->{interfaces}->%*], ['enp1s0', 'enp2s0'], 'VLAN links are dropped');
is_deeply($network->{interfaces}->{enp1s0}->{addresses}, [$address], 'addresses move to the link');
ok(!exists($network->{interfaces}->{enp2s0}->{addresses}), 'no addresses are made up');
is($network->{routes}->{gateway4}->{dev}, 'enp1s0', 'IPv4 default route uses the physical link');
is($network->{routes}->{gateway6}->{dev}, 'enp2s0', 'IPv6 default route uses the physical link');
is($network->{vlan_id}, 10, 'VLAN tag is recorded');

my $untagged = {
    interfaces => { enp1s0 => { name => 'enp1s0', addresses => [$address] } },
    routes => { gateway4 => { dev => 'enp1s0', gateway => '192.0.2.1' } },
};
Proxmox::Sys::Net::fold_live_vlan_links($untagged, {});

ok(!exists($untagged->{vlan_id}), 'no VLAN tag without VLAN links');
is($untagged->{routes}->{gateway4}->{dev}, 'enp1s0', 'routes stay as they are without VLAN links');

done_testing();
