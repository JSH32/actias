{
  description = "Actias: serverless Luau platform (development environment)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = f:
        nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          name = "actias";

          nativeBuildInputs = with pkgs; [
            # Rust workspace: worker, kv, script-service, cli, common.
            rustc
            cargo
            rustfmt
            clippy
            rust-analyzer

            # tonic-build / prost-build invoke protoc at build time.
            protobuf

            # actias-api and actias-web. Must match the node major used by the
            # images in docker/, so a native module that builds here also builds
            # in the container.
            nodejs_22

            # node-gyp needs python for the api's native deps (argon2), the
            # same reason docker/Dockerfile.api installs python3.
            python3

            # Task runner; see the justfile for the verb list.
            just

            # The smoke test drives the api with these.
            curl
            jq

            # reqwest/hyper-tls link against system openssl.
            pkg-config
          ];

          # mlua builds Luau from vendored C++ sources with the cc crate, which picks
          # its compiler up from the stdenv this shell already provides, so no extra
          # toolchain entry is needed for it.
          buildInputs = with pkgs; [
            openssl
          ];

          # Only greet a human; `nix develop -c ...` and CI stay quiet.
          #
          # Docker deliberately is not in this shell. A devshell can ship the client
          # but not dockerd, which needs cgroups, iptables and a system service, so
          # `just up` depends on a daemon the host provides.
          shellHook = ''
            if [ -t 1 ]; then
              echo "actias devshell: rustc $(rustc --version | cut -d' ' -f2), node $(node --version), protoc $(protoc --version | cut -d' ' -f2)"
              echo "run 'just' for available tasks"

              if ! command -v docker >/dev/null 2>&1; then
                echo "docker: not installed on this host, 'just up' will not work"
              elif ! docker info >/dev/null 2>&1; then
                # Membership must come from /etc/group, not `id -nG`: a session started
                # before the group was granted does not list it, which is precisely the
                # case this hint exists for.
                if getent group docker 2>/dev/null | cut -d: -f4 | tr ',' '\n' | grep -qx "$USER"; then
                  echo "docker: daemon unreachable, though you are in the docker group"
                  echo "        this shell predates that membership: re-login, or 'newgrp docker'"
                else
                  echo "docker: daemon unreachable, add your user to the docker group"
                fi
              fi
            fi
          '';
        };
      });
    };
}
