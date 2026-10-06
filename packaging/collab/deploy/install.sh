#!/usr/bin/env bash
# VectorCraft Cloud: install or update on a fresh Ubuntu 22.04/24.04 server, as root.
#
#   curl -fsSL https://raw.githubusercontent.com/milkpack/Claude/claude/cloud-illustrator-collab-bbf7yx/packaging/collab/deploy/install.sh | bash
#
# Options (environment variables):
#   DOMAIN=draw.example.com   your domain (its A record must point here); default: <ip>.sslip.io,
#                             a free name that resolves to this server, so HTTPS works without one
#   IMAGE=ghcr.io/...:tag     image to run (default: ghcr.io/milkpack/vectorcraft-cloud:latest)
#   GHCR_USER / GHCR_TOKEN    only if the image is private (a GitHub token with read:packages)
#
# Running it again updates to the latest image and keeps the documents.
set -euo pipefail

REPO_RAW="https://raw.githubusercontent.com/milkpack/Claude/claude/cloud-illustrator-collab-bbf7yx/packaging/collab/deploy"
DIR=/opt/vectorcraft
IMAGE="${IMAGE:-ghcr.io/milkpack/vectorcraft-cloud:latest}"

say() { printf '\n\033[1;32m==> %s\033[0m\n' "$*"; }
die() { printf '\n\033[1;31mОшибка: %s\033[0m\n' "$*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || die "запустите от root (или через sudo)"
command -v apt-get >/dev/null || die "нужен Ubuntu или Debian"

say "Пакеты"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq ca-certificates curl ufw cron >/dev/null

if ! command -v docker >/dev/null; then
  say "Docker"
  curl -fsSL https://get.docker.com | sh
fi
systemctl enable --now docker >/dev/null

say "Адрес"
IP="$(curl -fsS4 https://api.ipify.org || hostname -I | awk '{print $1}')"
DOMAIN="${DOMAIN:-${IP//./-}.sslip.io}"
echo "IP: $IP"
echo "Адрес сайта: https://$DOMAIN"

say "Файлы в $DIR"
mkdir -p "$DIR/data" "$DIR/backups"
curl -fsSL "$REPO_RAW/docker-compose.yml" -o "$DIR/docker-compose.yml"
curl -fsSL "$REPO_RAW/Caddyfile" -o "$DIR/Caddyfile"
cat > "$DIR/.env" <<EOF
DOMAIN=$DOMAIN
IMAGE=$IMAGE
EOF

say "Файрвол (SSH, HTTP, HTTPS)"
ufw allow OpenSSH >/dev/null
ufw allow 80/tcp >/dev/null
ufw allow 443/tcp >/dev/null
ufw --force enable >/dev/null

if [ -n "${GHCR_TOKEN:-}" ]; then
  say "Вход в GitHub Container Registry"
  echo "$GHCR_TOKEN" | docker login ghcr.io -u "${GHCR_USER:-milkpack}" --password-stdin
fi

say "Загрузка образа"
cd "$DIR"
if ! docker compose pull; then
  die "не удалось скачать $IMAGE.
  - Образ ещё собирается? Проверьте вкладку Actions: https://github.com/milkpack/Claude/actions
  - Образ приватный? Сделайте пакет публичным (GitHub → Packages → vectorcraft-cloud →
    Package settings → Change visibility → Public) или запустите с GHCR_TOKEN=..."
fi

say "Запуск"
docker compose up -d --remove-orphans
docker image prune -f >/dev/null

say "Ежедневный бэкап документов (03:30, хранится 14 дней)"
cat > /etc/cron.d/vectorcraft-backup <<EOF
30 3 * * * root tar -czf $DIR/backups/data-\$(date +\%F).tar.gz -C $DIR data && find $DIR/backups -name 'data-*.tar.gz' -mtime +14 -delete
EOF

say "Проверка"
for i in $(seq 1 30); do
  if docker compose exec -T vectorcraft true 2>/dev/null && curl -fsS -o /dev/null "http://127.0.0.1:80" -H "Host: $DOMAIN" 2>/dev/null; then
    break
  fi
  sleep 2
done
docker compose ps

cat <<EOF

Готово. VectorCraft Cloud работает:

    https://$DOMAIN/?room=team

Отправьте эту ссылку коллегам: все, кто открыл одну и ту же комнату (?room=...), рисуют вместе.
Первый запуск HTTPS-сертификата может занять до минуты.

Обновить до свежей версии:   запустите эту же команду ещё раз
Логи:                         cd $DIR && docker compose logs -f
Документы:                    $DIR/data   (бэкапы: $DIR/backups)
EOF
