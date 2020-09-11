NEWRELIC:=
CARGO_OPT=--release $(if $(NEWRELIC),--features use_newrelic,)
SYSTEMD_SERVICE_NAME=isucon.rust

.PHONY: run
run:
	cargo run $(CARGO_OPT)

.PHONY: build
build:
	cargo build $(CARGO_OPT)

.PHONY: check
check:
	cargo check $(CARGO_OPT)

.PHONY: log
log:
	journalctl -u $(SYSTEMD_SERVICE_NAME) -e

.PHONY: logf
logf:
	journalctl -u $(SYSTEMD_SERVICE_NAME) -f

.PHONY: alp
alp:
	# NOTE: Add -m '/xxx/.*,./yyy/.*' to aggreate logs
	alp ltsv --file /var/log/nginx/access.log r --sort=sum --filters="not(Uri startsWith '/netdata/')"

.PHONY: query-digest
query-digest:
	sudo pt-query-digest /tmp/mysql-slow.log | less
