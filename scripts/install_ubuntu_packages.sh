#!/usr/bin/env bash
set -euo pipefail

source /etc/os-release
sources=$(mktemp --suffix=.list)
trap 'rm -f "$sources"' EXIT
# Bypass the runner's Azure mirror list. Keep Ubuntu's signed archive and security updates.
keyring=/usr/share/keyrings/ubuntu-archive-keyring.gpg
cat > "$sources" <<EOF
deb [signed-by=$keyring] https://archive.ubuntu.com/ubuntu/ $VERSION_CODENAME main restricted universe multiverse
deb [signed-by=$keyring] https://archive.ubuntu.com/ubuntu/ $VERSION_CODENAME-updates main restricted universe multiverse
deb [signed-by=$keyring] https://security.ubuntu.com/ubuntu/ $VERSION_CODENAME-security main restricted universe multiverse
EOF
chmod 644 "$sources"

options=(-o "Dir::Etc::sourcelist=$sources" -o Dir::Etc::sourceparts=- -o Acquire::Retries=2 -o Acquire::https::Timeout=20)
sudo apt-get "${options[@]}" -o APT::Update::Error-Mode=any update
sudo apt-get "${options[@]}" install -y "$@"
