#!/bin/bash

# Check git
if ! git --version >/dev/null 2>&1; then
    echo "Git is not installed!"
    exit 1;
fi

# Check if cargo is installed
if ! cargo --version >/dev/null 2>&1; then
    echo "Rust is not installed, installing!"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
    source ~/.profile ~/.bashrc
fi

echo "Installing dependencies"
OS=$(awk -F= '/^NAME/{print $2}' /etc/os-release | tr -d '"')

# Ubuntu and Debian
if [ "$OS" = "Ubuntu" ] || [ "$OS" = "Debian" ]; then
    sudo apt-get update -y
    sudo apt-get install build-essential pkg-config libssl-dev -y
# Arch and Manjaro
elif [ "$OS" = "Arch Linux" ] || [ "$OS" = "Manjaro Linux" ]; then
    sudo pacman -Syu 
    sudo pacman -S base-devel pkgconf openssl
# Fedora
elif [ "$OS" = "Fedora" ]; then
    sudo dnf update -y
    sudo dnf groupinstall "Development Tools"
    sudo dnf install pkgconfig openssl-devel
else
    echo "Dependencies are not being handled for $OS. You need build tools, openssl, and pkgconf"
fi

if [ -d /tmp/actias-cli ]; then rm -Rf /tmp/actias-cli; fi

# Cloning actias
mkdir /tmp/actias-cli
cd /tmp/actias-cli
git clone https://github.com/JSH32/actias.git .

# Installing actias-cli
cargo install --path ./actias-cli