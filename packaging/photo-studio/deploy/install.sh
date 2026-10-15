#!/usr/bin/env bash
# Photo Studio: install or update on an Ubuntu 22.04/24.04 server, as root. It can share the
# server with VectorCraft Cloud (packaging/collab/deploy): own folder, port and Caddy site.
#
#   curl -L raw.githubusercontent.com/milkpack/Claude/HEAD/go-studio | sudo bash
#
# Options (environment variables; with sudo put them after it: `| sudo DOMAIN=... bash`):
#   DOMAIN=photo.example.com    your domain (its A record must point here); default: the one from the
#                               previous install, else photo.<ip>.sslip.io, a free name that resolves
#                               to this server, so HTTPS works without one
#   ANTHROPIC_API_KEY=sk-ant-…  enables the director and review nodes; default: the previous one
#   PHOTO_STUDIO_CLAUDE_MODEL=… Claude model (default: the server's, claude-opus-5-5)
#   PHOTO_STUDIO_PASSWORD=…     set a new owner password (default: keep the previous one; on the
#                               first install a strong random one is generated)
#   IMAGE=ghcr.io/...:tag       image to run (default: ghcr.io/milkpack/photo-studio:latest)
#   GHCR_USER / GHCR_TOKEN      only if the image is private (a GitHub token with read:packages)
#   REF=branch                  git ref to download docker-compose.yml from (default: HEAD)
#
# Running it again updates to the latest image and keeps the password, the Anthropic key, the
# OpenArt connection and every image (all in /opt/photo-studio).
#
# Caddy: the site lives in /etc/caddy/sites/photo-studio.caddy and the main Caddyfile only gets an
# `import /etc/caddy/sites/*.caddy` line, so a VectorCraft site already in it stays untouched.
# The VectorCraft installer rewrites /etc/caddy/Caddyfile and drops that line: after running it,
# run this script again (it restores the line and changes nothing else).
#
# Docker and Caddy come from Ubuntu's own repositories: get.docker.com and Docker Hub are often
# unreachable from Russian hosting, Ubuntu mirrors always are. Only our image comes from ghcr.io.
set -euo pipefail

# Everything also goes to a log, to send when asking for help. Secrets never go there: the final
# message with the password is printed to the terminal only (fd 3).
LOG=/var/log/photo-studio-install.log
exec 3>&1 4>&2
exec > >(tee -a "$LOG") 2>&1
TEE_PID=$!
echo "--- $(date -Is) install.sh"

REF="${REF:-HEAD}"
REPO_RAW="https://raw.githubusercontent.com/milkpack/Claude/$REF/packaging/photo-studio/deploy"
DIR=/opt/photo-studio
IMAGE="${IMAGE:-ghcr.io/milkpack/photo-studio:latest}"
PORT=8787
CADDYFILE=/etc/caddy/Caddyfile
SITES=/etc/caddy/sites
SITE="$SITES/photo-studio.caddy"
IMPORT_LINE="import $SITES/*.caddy"

say() { printf '\n\033[1;32m==> %s\033[0m\n' "$*"; }
warn() { printf '\033[1;33mВнимание: %s\033[0m\n' "$*"; }
die() {
  printf '\n\033[1;31mОшибка: %s\033[0m\n' "$*" >&2
  printf 'Полный лог: %s\n' "$LOG" >&2
  exit 1
}
# The raw value of KEY in the previous .env (empty if none).
env_get() {
  if [ -f "$DIR/.env" ]; then
    sed -n "s/^$1=//p" "$DIR/.env" | tail -n 1
  fi
}
# Values are written to .env unquoted, so keep them to characters docker compose reads literally.
safe_value() { [[ "$1" =~ ^[A-Za-z0-9._~@%+=,:/-]+$ ]]; }

[ "$(id -u)" -eq 0 ] || die "запустите от root (или через sudo)"
command -v apt-get >/dev/null || die "нужен Ubuntu или Debian"

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
# A subdomain of sslip.io: VectorCraft Cloud may already use <ip>.sslip.io on this server.
DOMAIN="${DOMAIN:-$(env_get DOMAIN)}"
DOMAIN="${DOMAIN:-photo.${IP//./-}.sslip.io}"
[[ "$DOMAIN" =~ ^[A-Za-z0-9.-]+$ ]] || die "DOMAIN=$DOMAIN: ожидается имя вида photo.example.com"
PUBLIC_URL="https://$DOMAIN"
echo "IP: $IP"
echo "Адрес сайта: $PUBLIC_URL"
echo "Вход в OpenArt вернётся именно на этот адрес ($PUBLIC_URL/oauth/openart/callback):"
echo "открывайте Photo Studio только по нему. Сменили адрес — подключите OpenArt заново."

say "Настройки в $DIR/.env"
mkdir -p "$DIR/data" "$DIR/backups"
chmod 700 "$DIR/backups"
NEW_PASSWORD=""
if [ -n "${PHOTO_STUDIO_PASSWORD:-}" ]; then
  safe_value "$PHOTO_STUDIO_PASSWORD" || die "PHOTO_STUDIO_PASSWORD: только латиница, цифры и ._~@%+=,:/-"
  [ "${#PHOTO_STUDIO_PASSWORD}" -ge 12 ] || die "PHOTO_STUDIO_PASSWORD: нужно не меньше 12 символов"
  PASSWORD="$PHOTO_STUDIO_PASSWORD"
  NEW_PASSWORD=1
  echo "Пароль: новый, из PHOTO_STUDIO_PASSWORD"
else
  PASSWORD="$(env_get PHOTO_STUDIO_PASSWORD)"
  if [ -z "$PASSWORD" ]; then
    # ~190 bits: 32 characters of base64 without + and /.
    PASSWORD="$(head -c 48 /dev/urandom | base64 | tr -dc 'A-Za-z0-9')"
    PASSWORD="${PASSWORD:0:32}"
    NEW_PASSWORD=1
    echo "Пароль: создан новый (будет показан в конце)"
  else
    echo "Пароль: прежний"
  fi
fi
if [ -n "${ANTHROPIC_API_KEY:-}" ]; then
  [[ "$ANTHROPIC_API_KEY" =~ ^[A-Za-z0-9_-]+$ ]] || die "ANTHROPIC_API_KEY выглядит неправильно (ожидается sk-ant-...)"
  API_KEY="$ANTHROPIC_API_KEY"
  echo "Ключ Anthropic: из ANTHROPIC_API_KEY"
else
  API_KEY="$(env_get ANTHROPIC_API_KEY)"
  if [ -n "$API_KEY" ]; then
    echo "Ключ Anthropic: прежний"
  else
    warn "ключа Anthropic нет: узлы Director и Review выключены, генерация через OpenArt работает.
  Включить: запустите установку ещё раз с ANTHROPIC_API_KEY=sk-ant-..."
  fi
fi
MODEL="${PHOTO_STUDIO_CLAUDE_MODEL:-$(env_get PHOTO_STUDIO_CLAUDE_MODEL)}"
[ -z "$MODEL" ] || [[ "$MODEL" =~ ^[A-Za-z0-9._-]+$ ]] || die "PHOTO_STUDIO_CLAUDE_MODEL=$MODEL: ожидается имя модели, например claude-opus-5-5"
EFFORT="$(env_get PHOTO_STUDIO_CLAUDE_EFFORT)"

curl -fsSL --max-time 30 "$REPO_RAW/docker-compose.yml" -o "$DIR/docker-compose.yml" ||
  die "не удалось скачать docker-compose.yml с raw.githubusercontent.com"
# Written to a temp file with 0600 and renamed: the password and the key never sit world-readable.
(
  umask 077
  {
    printf '# Photo Studio settings (install.sh keeps them on re-runs). Apply edits: docker compose up -d\n'
    printf 'DOMAIN=%s\nIMAGE=%s\n' "$DOMAIN" "$IMAGE"
    printf 'PHOTO_STUDIO_PUBLIC_URL=%s\n' "$PUBLIC_URL"
    printf 'PHOTO_STUDIO_PASSWORD=%s\n' "$PASSWORD"
    if [ -n "$API_KEY" ]; then printf 'ANTHROPIC_API_KEY=%s\n' "$API_KEY"; else printf '#ANTHROPIC_API_KEY=sk-ant-...\n'; fi
    if [ -n "$MODEL" ]; then printf 'PHOTO_STUDIO_CLAUDE_MODEL=%s\n' "$MODEL"; else printf '#PHOTO_STUDIO_CLAUDE_MODEL=claude-opus-5-5\n'; fi
    if [ -n "$EFFORT" ]; then printf 'PHOTO_STUDIO_CLAUDE_EFFORT=%s\n' "$EFFORT"; else printf '#PHOTO_STUDIO_CLAUDE_EFFORT=medium\n'; fi
  } > "$DIR/.env.new"
)
chmod 600 "$DIR/.env.new"
mv -f "$DIR/.env.new" "$DIR/.env"
(cd "$DIR" && docker compose config -q) || die "docker-compose.yml или .env не прошли проверку: cd $DIR && docker compose config -q"

say "Caddy (HTTPS): сайт в $SITE"
mkdir -p "$SITES"
# Keep the current files, to put them back if the new configuration doesn't validate.
BACKUP="$(mktemp -d)"
[ -f "$CADDYFILE" ] && cp -a "$CADDYFILE" "$BACKUP/Caddyfile"
[ -f "$SITE" ] && cp -a "$SITE" "$BACKUP/site"
restore_caddy() {
  if [ -f "$BACKUP/Caddyfile" ]; then cp -a "$BACKUP/Caddyfile" "$CADDYFILE"; else rm -f "$CADDYFILE"; fi
  if [ -f "$BACKUP/site" ]; then cp -a "$BACKUP/site" "$SITE"; else rm -f "$SITE"; fi
}
cat > "$SITE" <<EOF
# Photo Studio (written by packaging/photo-studio/deploy/install.sh; re-running it rewrites this file)
$DOMAIN {
	encode zstd gzip
	reverse_proxy 127.0.0.1:$PORT
}
EOF
# The Ubuntu package's placeholder Caddyfile serves a welcome page on :80 for every host: replace
# it. Any other Caddyfile (VectorCraft's, or hand-written) is kept and only gets the import line.
STOCK=":80{root*/usr/share/caddyfile_server}"
if [ ! -s "$CADDYFILE" ] || [ "$(grep -v '^[[:space:]]*#' "$CADDYFILE" | tr -d '[:space:]')" = "$STOCK" ]; then
  printf '# Every site is a file in %s (written by the installers).\n%s\n' "$SITES" "$IMPORT_LINE" > "$CADDYFILE"
  echo "Caddyfile: заменён стандартный из пакета"
elif grep -Eq "^[[:space:]]*import[[:space:]]+$SITES/\*\.caddy[[:space:]]*$" "$CADDYFILE"; then
  echo "Caddyfile: строка import уже есть"
else
  printf '\n# Sites in %s (Photo Studio and others). The VectorCraft installer drops this line:\n# re-run the Photo Studio installer to restore it.\n%s\n' "$SITES" "$IMPORT_LINE" >> "$CADDYFILE"
  echo "Caddyfile: добавлена строка $IMPORT_LINE"
fi
if ! CHECK="$(caddy validate --config "$CADDYFILE" --adapter caddyfile 2>&1)"; then
  printf '%s\n' "$CHECK" | tail -n 5
  restore_caddy
  die "конфигурация Caddy не прошла проверку, прежние файлы возвращены.
  Адрес $DOMAIN уже занят другим сайтом на этом сервере? Задайте другой: DOMAIN=photo.example.com"
fi
rm -rf "$BACKUP"

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
# Reload, not restart: a VectorCraft site on the same Caddy keeps serving.
systemctl reload-or-restart caddy

say "Ежедневный бэкап данных (03:45, хранится 14 дней)"
# shellcheck disable=SC2016 # $(date ...) is for cron to expand, not this script
printf '45 3 * * * root tar -czf %s/backups/data-$(date +\\%%F).tar.gz -C %s data && find %s/backups -name "data-*.tar.gz" -mtime +14 -delete\n' \
  "$DIR" "$DIR" "$DIR" > /etc/cron.d/photo-studio-backup

say "Проверка"
OK=""
for _ in $(seq 1 30); do
  if curl -fsS "http://127.0.0.1:$PORT/healthz" >/dev/null 2>&1; then
    OK=1
    break
  fi
  sleep 2
done
docker compose ps
[ -n "$OK" ] || die "приложение не отвечает. Логи: cd $DIR && docker compose logs --tail 50"
systemctl is-active --quiet caddy || die "Caddy не запустился. Логи: journalctl -u caddy --no-pager -n 50"

summary() {
  local claude
  if [ -n "$API_KEY" ]; then
    claude="включены (ключ Anthropic задан)"
  else
    claude="ВЫКЛЮЧЕНЫ: нет ключа. Запустите установку ещё раз с ANTHROPIC_API_KEY=sk-ant-..."
  fi
  cat <<EOF

Готово. Photo Studio работает:

    $PUBLIC_URL

Пароль владельца:   $1
Director и Review:  $claude

Следующий шаг: откройте сайт, войдите с паролем, затем Settings → Connect OpenArt.
Вход в OpenArt работает только по адресу $PUBLIC_URL.
Первый выпуск HTTPS-сертификата может занять до минуты.

Обновить до свежей версии:   запустите эту же команду ещё раз
Логи приложения:              cd $DIR && docker compose logs -f
Логи Caddy (HTTPS):           journalctl -u caddy -f
Лог установки:                $LOG
Настройки:                    $DIR/.env   (пароль и ключ; только для root)
Данные:                       $DIR/data   (бэкапы: $DIR/backups)
EOF
}

# The password goes to the terminal only: stop the log first, then write a copy without it.
exec >&3 2>&4
wait "$TEE_PID" 2>/dev/null || true
if [ -n "$NEW_PASSWORD" ]; then
  summary "$PASSWORD   (новый: сохраните его, он же лежит в $DIR/.env)"
  summary "(скрыт: см. $DIR/.env)" >> "$LOG"
else
  summary "$PASSWORD   (прежний)"
  summary "(прежний; скрыт: см. $DIR/.env)" >> "$LOG"
fi
