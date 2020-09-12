NEWRELIC:=
# TODO: Enable newrelic after embedding newrelic code to app.
CARGO_OPT=--release --locked #$(if $(NEWRELIC),--features use_newrelic,)
SYSTEMD_SERVICE_NAME=isucon.rust

rundev:
	# systemfd and cargo-watch is required for autoloading
	# % cargo systemfd cargo-watch
	RUST_LOG=info systemfd --no-pid --socket http::1323 -- cargo watch -x run

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