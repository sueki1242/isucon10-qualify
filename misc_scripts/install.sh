#!/bin/bash
set -eux


function install_alp() {
    wget https://github.com/tkuchiki/alp/releases/download/v1.0.3/alp_linux_amd64.zip
    unzip alp_linux_amd64.zip
    sudo install ./alp /usr/local/bin
    rm alp
    rm alp_linux_amd64.zip
}

function install_pt_query_digest() {
    curl -L https://github.com/percona/percona-toolkit/archive/3.0.5-test.tar.gz | tar zxv
    ./percona-toolkit-3.0.5-test/bin/pt-query-digest --version
    sudo mv ./percona-toolkit-3.0.5-test/bin/pt-query-digest /usr/local/bin/pt-query-digest
    rm -rf percona-toolkit-3.0.5-test
}

function install_newrelic() {
    cd
    git clone https://github.com/newrelic/c-sdk
    cd c-sdk
    make
    sudo mkdir -p /var/log/newrelic
    sudo chmod a+w /var/log/newrelic

    ./newrelic-daemon
}

function install_netdata() {
    bash <(curl -Ss https://my-netdata.io/kickstart.sh) --no-updates --stable-channel --disable-telemetry

    sudo sh -c "cat >/etc/nginx/sites-enabled/netdata.conf" <<EOH
    server {
        location /netdata/ {
            proxy_pass http://127.0.0.1:19999/;
        }
    }
EOH
}

sudo apt install -y unzip libpcre3-dev git

install_alp &
install_pt_query_digest &
install_newrelic &
wait

# netdata installation requires user input
install_netdata
