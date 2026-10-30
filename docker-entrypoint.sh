#!/bin/sh
set -eu

URL="https://github.com/KlevisImeri/TodoBot/releases/latest/download/todobot.rpm"

if ! dnf install -y --nogpgcheck "$URL"; then
    echo "error: failed to install todobot from $URL" >&2
    exit 1
fi

HOME=/home/todobot
export HOME

exec setpriv --reuid=todobot --regid=todobot --init-groups todobot
