all:

export PERLLIB=..

.PHONY: check
check: test-zfs-arc-max test-run-command test-parse-fqdn test-ui2-stdio \
       test-zfs-get-pool-list test-parse-kernel-cmdline \
       test-validate-link-pin-map test-generate-ifupdown2-config test-live-vlan-links

.PHONY: test-zfs-arc-max
test-zfs-arc-max:
	./zfs-arc-max.pl

.PHONY: test-run-command
test-run-command:
	./run-command.pl

.PHONY: test-parse-fqdn
test-parse-fqdn:
	./parse-fqdn.pl

.PHONY: test-ui2-stdio
test-ui2-stdio:
	./ui2-stdio.pl

.PHONY: test-zfs-get-pool-list
test-zfs-get-pool-list:
	./zfs-get-pool-list.pl

.PHONY: test-parse-kernel-cmdline
test-parse-kernel-cmdline:
	./parse-kernel-cmdline.pl

.PHONY: test-validate-link-pin-map
test-validate-link-pin-map:
	./validate-link-pin-map.pl

.PHONY: test-generate-ifupdown2-config
test-generate-ifupdown2-config:
	./generate-ifupdown2-config.pl

.PHONY: test-live-vlan-links
test-live-vlan-links:
	./live-vlan-links.pl
