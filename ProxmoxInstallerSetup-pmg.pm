package ProxmoxInstallerSetup;

use strict;
use warnings;

sub setup {
    return {
	product => 'pmg',
	enable_btrfs => 0,
    };
}

1;


