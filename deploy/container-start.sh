#!/bin/sh
set -eu
: "${GIGAAM_SERVER_TOKEN:?Set GIGAAM_SERVER_TOKEN}"
case "$GIGAAM_SERVER_TOKEN" in *[!a-zA-Z0-9_-]*) echo 'Token must be URL-safe alphanumeric' >&2; exit 1;; esac
cat > /etc/nginx/sites-enabled/default <<EOF
server {
 listen 80;
 root /usr/share/nginx/html;
 client_max_body_size 64m;
 location / { try_files \$uri \$uri/ /index.html; }
 location /api/ {
   proxy_pass http://127.0.0.1:8787/;
   proxy_set_header Authorization "Bearer ${GIGAAM_SERVER_TOKEN}";
   proxy_http_version 1.1;
   proxy_set_header Upgrade \$http_upgrade;
   proxy_set_header Connection "upgrade";
   proxy_read_timeout 600s;
 }
}
EOF
su -s /bin/sh www-data -c gigaam-server &
backend=$!
trap 'kill "$backend" 2>/dev/null || true' EXIT INT TERM
nginx -g 'daemon off;'
