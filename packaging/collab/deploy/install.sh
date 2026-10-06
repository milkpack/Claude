#!/usr/bin/env bash
# VectorCraft Cloud: install or update on a fresh Ubuntu 22.04/24.04 server, as root.
#
#   curl -L raw.githubusercontent.com/milkpack/Claude/HEAD/go | bash
#
# Options (environment variables):
#   DOMAIN=draw.example.com   your domain (its A record must point here); default: <ip>.sslip.io,
#                             a free name that resolves to this server, so HTTPS works without one
#   IMAGE=ghcr.io/...:tag     image to run (default: ghcr.io/milkpack/vectorcraft-cloud:latest)
#   GHCR_USER / GHCR_TOKEN    only if the image is private (a GitHub token with read:packages)
#
# Running it again updates to the latest image and keeps the documents.
#
# Docker and Caddy come from Ubuntu's own repositories: get.docker.com and Docker Hub are often
# unreachable from Russian hosting, Ubuntu mirrors always are. Only our image comes from ghcr.io.
set -euo pipefail

# Everything also goes to a log, to send when asking for help.
LOG=/var/log/vectorcraft-install.log
exec > >(tee -a "$LOG") 2>&1
echo "--- $(date -Is) install.sh"

REPO_RAW="https://raw.githubusercontent.com/milkpack/Claude/HEAD/packaging/collab/deploy"
DIR=/opt/vectorcraft
IMAGE="${IMAGE:-ghcr.io/milkpack/vectorcraft-cloud:latest}"

say() { printf '\n\033[1;32m==> %s\033[0m\n' "$*"; }
die() {
  printf '\n\033[1;31mОшибка: %s\033[0m\n' "$*" >&2
  printf 'Полный лог: %s\n' "$LOG" >&2
  exit 1
}

[ "$(id -u)" -eq 0 ] || die "запустите от root (или через sudo)"
command -v apt-get >/dev/null || die "нужен Ubuntu или Debian"

# A previous install may run Caddy in a container on ports 80/443: stop that stack first.
if [ -f "$DIR/docker-compose.yml" ] && command -v docker >/dev/null; then
  say "Остановка прежней установки"
  (cd "$DIR" && docker compose down --remove-orphans) || true
fi

say "Пакеты: Docker, Caddy (из репозиториев Ubuntu)"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq || die "apt-get update не прошёл: нет доступа к репозиториям Ubuntu?"
apt-get install -y -qq ca-certificates curl ufw cron caddy >/dev/null || die "не удалось поставить пакеты"
command -v docker >/dev/null || apt-get install -y -qq docker.io >/dev/null || die "не удалось поставить Docker"
docker compose version >/dev/null 2>&1 || apt-get install -y -qq docker-compose-v2 >/dev/null ||
  die "не удалось поставить docker compose"
systemctl enable --now docker >/dev/null

say "Адрес"
IP="$(curl -fsS4 --max-time 10 https://api.ipify.org || hostname -I | awk '{print $1}')"
DOMAIN="${DOMAIN:-${IP//./-}.sslip.io}"
echo "IP: $IP"
echo "Адрес сайта: https://$DOMAIN"

say "Файлы в $DIR"
mkdir -p "$DIR/data" "$DIR/backups"
curl -fsSL --max-time 30 "$REPO_RAW/docker-compose.yml" -o "$DIR/docker-compose.yml" ||
  die "не удалось скачать docker-compose.yml с raw.githubusercontent.com"
printf 'DOMAIN=%s\nIMAGE=%s\n' "$DOMAIN" "$IMAGE" > "$DIR/.env"
# Caddy runs on the host and proxies to the app on 127.0.0.1:1234 (HTTPS certificate included).
# The web app is a ~40 MB .wasm: compress it (Caddy leaves application/wasm alone by default) and
# let browsers keep the hashed build files for good, so it downloads once per release.
cat > /etc/caddy/Caddyfile <<EOF
$DOMAIN {
	encode {
		zstd
		gzip 6
		match {
			header Content-Type application/wasm*
			header Content-Type text/*
			header Content-Type application/javascript*
			header Content-Type application/json*
			header Content-Type image/svg+xml*
		}
	}
	@hashed path_regexp \-[0-9a-f]{12,}(_bg)?\.(wasm|js)$
	header @hashed {
		Cache-Control "public, max-age=31536000, immutable"
		defer
	}
	reverse_proxy 127.0.0.1:1234
}
EOF
caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile >/dev/null 2>&1 ||
  die "Caddyfile не прошёл проверку: caddy validate --config /etc/caddy/Caddyfile"

say "Файрвол (SSH, HTTP, HTTPS)"
ufw allow OpenSSH >/dev/null
ufw allow 80/tcp >/dev/null
ufw allow 443/tcp >/dev/null
ufw --force enable >/dev/null

if [ -n "${GHCR_TOKEN:-}" ]; then
  say "Вход в GitHub Container Registry"
  echo "$GHCR_TOKEN" | docker login ghcr.io -u "${GHCR_USER:-milkpack}" --password-stdin
fi

say "Загрузка образа $IMAGE"
cd "$DIR"
docker compose pull || die "не удалось скачать $IMAGE с ghcr.io.
  - Образ ещё собирается? Проверьте вкладку Actions: https://github.com/milkpack/Claude/actions
  - Образ приватный? Сделайте пакет публичным или запустите с GHCR_TOKEN=..."

say "Запуск"
docker compose up -d --remove-orphans
docker image prune -f >/dev/null || true
systemctl enable caddy >/dev/null
systemctl restart caddy

say "Ежедневный бэкап документов (03:30, хранится 14 дней)"
printf '30 3 * * * root tar -czf %s/backups/data-$(date +\\%%F).tar.gz -C %s data && find %s/backups -name "data-*.tar.gz" -mtime +14 -delete\n' \
  "$DIR" "$DIR" "$DIR" > /etc/cron.d/vectorcraft-backup

say "Проверка"
OK=""
for _ in $(seq 1 30); do
  if curl -fsS http://127.0.0.1:1234/api/health >/dev/null 2>&1; then
    OK=1
    break
  fi
  sleep 2
done
docker compose ps
[ -n "$OK" ] || die "приложение не отвечает. Логи: cd $DIR && docker compose logs --tail 50"
systemctl is-active --quiet caddy || die "Caddy не запустился. Логи: journalctl -u caddy --no-pager -n 50"

cat <<EOF

Готово. VectorCraft Cloud работает:

    https://$DOMAIN/?room=team

Отправьте эту ссылку коллегам: все, кто открыл одну и ту же комнату (?room=...), рисуют вместе.
Первый выпуск HTTPS-сертификата может занять до минуты.

Обновить до свежей версии:   запустите эту же команду ещё раз
Логи приложения:              cd $DIR && docker compose logs -f
Логи Caddy (HTTPS):           journalctl -u caddy -f
Лог установки:                $LOG
Документы:                    $DIR/data   (бэкапы: $DIR/backups)
EOF
