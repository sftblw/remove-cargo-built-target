#!/bin/bash

# Note: https://graphicdesign.stackexchange.com/a/110023

set -ex

svg="./icon.svg"

# Array of image sizes (adjust the order if needed)
size=(16 32 48 64 128 256)

# Create a temporary directory using mktemp -d
out=$(mktemp -d)

echo "Converting SVG to bitmap images..."

for i in "${size[@]}"; do
  # For recent versions of Inkscape, use the --export-filename, --export-width, and --export-height options.
  inkscape "$svg" --export-filename="$out/${i}.png" --export-width="$i" --export-height="$i"
done

echo "Compressing images..."

# Loop through each file to compress without using glob expansion
for file in "$out"/*.png; do
  pngquant -f --ext .png "$file" --posterize 4 --speed 1
done

echo "Copying 256x256 PNG to icon.png..."
cp "$out/256.png" ../icon.png

echo "Converting PNG files to icon.ico..."

# Convert multiple PNG files into a single icon.ico file.
magick "$out/*.png" ../favicon.ico

# Remove the temporary directory
rm -rf "$out/"

echo "Operation completed!"
