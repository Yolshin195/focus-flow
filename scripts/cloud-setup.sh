#!/usr/bin/env bash
# Подготовка облачной сессии Claude Code (хук SessionStart из .claude/settings.json).
# Локально ничего не делает. Повторный запуск безопасен: готовое пропускается.
#
# Всё скачивается готовыми бинарниками через curl. Ничего не собирается из исходников:
# - cargo-binstall ищет бинарник dx через GitHub API, прокси облака отвечает на него 403,
#   и binstall молча собирает dx из исходников (~25 минут);
# - сам dx не доверяет сертификату прокси (свои корневые сертификаты, SSL_CERT_FILE
#   не читает): wasm-bindgen он собирает из исходников, а esbuild не скачивает вовсе,
#   и web-сборка падает. Поэтому оба кладём туда, где dx ищет их до скачивания.
set -uo pipefail
[ "${CLAUDE_CODE_REMOTE:-}" = "true" ] || exit 0

cd "${CLAUDE_PROJECT_DIR:-$(dirname "$0")/..}" || exit 1
failed=0
warn() { echo "cloud-setup: $*" >&2; failed=1; }

# Версия пакета из Cargo.lock.
lock_version() {
  awk -v name="$1" '$0 == "name = \"" name "\"" { getline; gsub(/"/, "", $3); print $3; exit }' Cargo.lock
}

# fetch_bin <url архива .tar.gz> <путь файла в архиве> <куда положить>
fetch_bin() {
  local tmp ok=1
  tmp=$(mktemp -d)
  if curl -fsSL --retry 3 -o "$tmp/a.tgz" "$1" && tar -xzf "$tmp/a.tgz" -C "$tmp" "$2"; then
    mkdir -p "$(dirname "$3")" && install -m 755 "$tmp/$2" "$3" && ok=0
  fi
  rm -rf "$tmp"
  return "$ok"
}

# Сборка без отладочной информации: быстрее и меньше места на диске.
if [ -n "${CLAUDE_ENV_FILE:-}" ]; then
  echo 'export CARGO_PROFILE_DEV_DEBUG=0' >>"$CLAUDE_ENV_FILE"
fi

# dx — той же версии, что dioxus в Cargo.lock.
dx_version=$(lock_version dioxus)
dx_bin="${CARGO_HOME:-$HOME/.cargo}/bin/dx"
if [ -z "$dx_version" ]; then
  warn "не нашёл версию dioxus в Cargo.lock — dx не установлен"
elif [ "$("$dx_bin" --version 2>/dev/null | awk '{ print $2 }')" != "$dx_version" ]; then
  url="https://github.com/DioxusLabs/dioxus/releases/download/v${dx_version}/dx-x86_64-unknown-linux-gnu.tar.gz"
  if fetch_bin "$url" dx "$dx_bin"; then
    echo "cloud-setup: dx $dx_version установлен"
  else
    warn "не удалось скачать dx $dx_version ($url). Не собирай dx из исходников (cargo install / binstall) — это ~25 минут."
  fi
fi

# Инструменты dx для web-сборки — в его каталог ~/.local/share/.dx/tools/<имя>-<версия>/.
dx_tools="${DX_HOME:-${XDG_DATA_HOME:-$HOME/.local/share}/.dx}/tools"

# wasm-bindgen — той же версии, что крейт wasm-bindgen в Cargo.lock (dx это проверяет).
wb_version=$(lock_version wasm-bindgen)
if [ -n "$wb_version" ] && [ ! -x "$dx_tools/wasm-bindgen-$wb_version/wasm-bindgen" ]; then
  wb_name="wasm-bindgen-${wb_version}-x86_64-unknown-linux-musl"
  url="https://github.com/wasm-bindgen/wasm-bindgen/releases/download/${wb_version}/${wb_name}.tar.gz"
  if fetch_bin "$url" "$wb_name/wasm-bindgen" "$dx_tools/wasm-bindgen-$wb_version/wasm-bindgen"; then
    echo "cloud-setup: wasm-bindgen $wb_version установлен"
  else
    warn "не удалось скачать wasm-bindgen $wb_version ($url)"
  fi
fi

# esbuild — версию dx задаёт константой ESBUILD_VERSION в своих исходниках.
if [ -n "$dx_version" ] && ! compgen -G "$dx_tools/esbuild-*/.for-dx-$dx_version" >/dev/null; then
  src="https://raw.githubusercontent.com/DioxusLabs/dioxus/v${dx_version}/packages/cli/src/esbuild.rs"
  es_version=$(curl -fsSL --retry 3 "$src" | sed -n 's/^const ESBUILD_VERSION: &str = "\(.*\)";$/\1/p')
  url="https://registry.npmjs.org/@esbuild/linux-x64/-/linux-x64-${es_version}.tgz"
  if [ -z "$es_version" ]; then
    warn "не узнал версию esbuild для dx $dx_version ($src)"
  elif fetch_bin "$url" package/bin/esbuild "$dx_tools/esbuild-$es_version/esbuild"; then
    touch "$dx_tools/esbuild-$es_version/.for-dx-$dx_version"
    echo "cloud-setup: esbuild $es_version установлен"
  else
    warn "не удалось скачать esbuild $es_version ($url)"
  fi
fi

# Системные библиотеки для desktop: сборка (wry — WebKitGTK, muda и global-hotkey — libxdo)
# и трей при запуске (tray-icon подгружает libayatana-appindicator3 через dlopen; без неё
# окно работает, но без трея).
desktop_pkgs=(libwebkit2gtk-4.1-dev libgtk-3-dev libxdo-dev libayatana-appindicator3-1)
if ! pkg-config --exists webkit2gtk-4.1 gtk+-3.0 2>/dev/null || [ ! -e /usr/include/xdo.h ] ||
  ! compgen -G "/usr/lib/*/libayatana-appindicator3.so.1" >/dev/null; then
  sudo=""
  [ "$(id -u)" -eq 0 ] || sudo="sudo"
  if $sudo apt-get update -qq >/dev/null 2>&1 &&
    $sudo env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
      "${desktop_pkgs[@]}" >/dev/null 2>&1; then
    echo "cloud-setup: системные библиотеки для desktop установлены"
  else
    warn "apt-get не смог поставить ${desktop_pkgs[*]} — desktop не соберётся или запустится без трея"
  fi
fi

# Тулчейн из rust-toolchain.toml (с целью wasm32) и исходники всех зависимостей.
cargo fetch --locked -q || warn "cargo fetch не прошёл"

exit "$failed"
