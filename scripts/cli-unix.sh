#!/bin/bash

# Cloning actias
mkdir /tmp/actias-cli
cd /tmp/actias-cli
git clone https://github.com/JSH32/actias.git .

# Installing actias-cli
cargo install --path ./actias-cli