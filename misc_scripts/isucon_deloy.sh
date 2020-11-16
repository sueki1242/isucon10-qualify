#!/bin/bash 

set -e

: configuration
GIT_DIR=$HOME
BUILD_DIR_FROM_TOP=isuumo/webapp/rust

SYSTEMD_SERVICE_NAME=isuumo.rust

NEWRELIC_API_KEY=${newrelic_key}
NEWRELIC_APPLICATION_ID=${app_id}

MYSQL_USER=isucon
MYSQL_PASS=isucon

time=$(date +%H%M%S)
level=${1:-1}

: check
if [ -n "$(grep /var/log/nginx/access.log /etc/nginx/nginx.conf)" ]; then
  echo "/etc/nginx/nginx.conf からaccess_logの設定を除いてください"
  exit 1
fi


: git pull
(
  cd ${GIT_DIR}
  git pull
)

: ビルド
(
  cd ${GIT_DIR}/${BUILD_DIR_FROM_TOP}
  if (( ${level} >= 1 )); then
    make build
  else
    make build NEWRELIC=1
  fi
)

# デプロイ → 省略？

: Mysqlのログ切り替え
(
  cd /etc/mysql/
  if [ -f /tmp/mysql-slow.log ]; then
    sudo mv /tmp/mysql-slow.log /tmp/mysql-slow-${time}.log
  fi
  
  if (( ${level} >= 1 )); then
    echo "set global slow_query_log = OFF;FLUSH LOGS;" | mysql -u ${MYSQL_USER} -p${MYSQL_PASS}
  else
    echo "set global slow_query_log_file = '/tmp/mysql-slow.log'; set global long_query_time=0; set global slow_query_log = ON; FLUSH LOGS;" | mysql -u ${MYSQL_USER} -p${MYSQL_PASS}
  fi
)

: NGINXのログ切り替え
(
  cd /etc/nginx/
  if [ -f /var/log/nginx/access.log ]; then
    sudo mv /var/log/nginx/access.log /tmp/nginx-access-${time}.log
  fi
  if [ ! -f disable-log.conf ]; then
    sudo sh -c "cat >disable-log.conf" <<EOH
        access_log off;
        error_log /dev/null;
EOH
  fi
  if [ ! -f enable-log.conf ]; then
    sudo sh -c "cat >enable-log.conf" <<'EOH' 
        log_format ltsv "time:$time_local"
                "\thost:$remote_addr"
                "\tforwardedfor:$http_x_forwarded_for"
                "\treq:$request"
                "\tstatus:$status"
                "\tmethod:$request_method"
                "\turi:$request_uri"
                "\tsize:$body_bytes_sent"
                "\treferer:$http_referer"
                "\tua:$http_user_agent"
                "\treqtime:$request_time"
                "\tcache:$upstream_http_x_cache"
                "\truntime:$upstream_http_x_runtime"
                "\tapptime:$upstream_response_time"
                "\tvhost:$host";
        access_log /var/log/nginx/access.log ltsv;
        error_log /var/log/nginx/error.log;
EOH
  fi

  if (( ${level} >= 1 )); then
    sudo cp disable-log.conf conf.d/ && sudo rm -f conf.d/enable-log.conf
  else
    sudo cp enable-log.conf conf.d/ && sudo rm -f conf.d/disable-log.conf
  fi
  sudo systemctl restart nginx
)

: サービス再起動
sudo systemctl restart ${SYSTEMD_SERVICE_NAME}

: Release marker更新
(
  cd ${GIT_DIR}
  rev=$(git rev-parse HEAD)
  message=$(git show -s --format='%B' ${rev} | head -1 | sed -e "s/\"/'/g")
  user=$(git show -s --format='%ae' ${rev})
  curl -X POST \
      -H "X-Api-Key: ${NEWRELIC_API_KEY}" \
      -H "Content-Type: application/json" \
      -d "{ \"deployment\": { \"revision\": \"${rev}\", \"changelog\": \"${message}\", \"description\": \"${message}\", \"user\": \"${user}\" } }" \
      "https://api.newrelic.com/v2/applications/${NEWRELIC_APPLICATION_ID}/deployments.json"

  # 時系列で分かるようにDeploy markerのログと時刻出力
  echo "${time} ${rev}" >>/tmp/deploy.log
)

# TODO: もう少し巧妙にする
echo ${level} >/tmp/.level

if [ ! -f ~/.bashrc_prompt ]; then
  cat <<'EOH' >~/.bashrc_prompt
function get_level() {
  echo "(level:$(cat /tmp/.level))"
}
function proml {
  PS1="[\u@\h \W\$(get_level)]\$ "
}
proml
EOH
fi

if [ -z "$(grep .bashrc_prompt ~/.bashrc)" ]; then
  echo "source ~/.bashrc_prompt" >>~/.bashrc
fi

: ベンチ実行
## TODO: 
# curl -X POST -H "Cookie: ${ISUCON_PORTAL_COOKIE}" \
#      -d "servername=xxxxx" \
#      https://portal.isucon.net/........./bench
