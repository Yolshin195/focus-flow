#!/bin/sh
# Иконки трея из SVG (src/*.svg) в PNG. Нужен rsvg-convert (librsvg).
#
#   <фаза>.png, <фаза>@2x.png — 18 и 36 px, чёрные с альфой: template-иконки
#                               строки меню macOS (система сама красит их под тему);
#   color/<фаза>.png          — 36 px в цвете сессии из tokens.css: для трея
#                               Linux и Windows, где панель бывает и светлой, и тёмной.
set -eu
cd "$(dirname "$0")"
mkdir -p color
color() {
    case "$1" in
        work) echo "#5b8cff" ;;
        rest) echo "#3dd68c" ;;
        planning) echo "#f5b841" ;;
        *) echo "#9aa1ad" ;;
    esac
}
for svg in src/*.svg; do
    name=$(basename "$svg" .svg)
    rsvg-convert -w 18 -h 18 "$svg" -o "$name.png"
    rsvg-convert -w 36 -h 36 "$svg" -o "$name@2x.png"
    css=$(mktemp)
    echo "svg { color: $(color "$name"); }" > "$css"
    rsvg-convert -w 36 -h 36 --stylesheet "$css" "$svg" -o "color/$name.png"
    rm -f "$css"
done
