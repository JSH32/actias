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

            # The workbench's Luau language service is a wasm target built
            # from Luau's own analysis library; see luau-web/README.md for
            # the build. Only needed to regenerate that artifact, which is
            # vendored, so an ordinary checkout never runs this.
            emscripten
            cmake
            ninja

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
            # `actias check` shells out to luau-analyze for typed lua.
            luau
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

        # The chart work (TODO R1b through R2b, spec in docs/HELM.md) is verified
        # on kind, so kind is what this shell carries. It stays a separate shell
        # rather than growing the default one because these are a few hundred
        # megabytes of Go binaries an ordinary checkout never needs; enter it
        # with `nix develop .#kube`. inputsFrom pulls the default shell's whole
        # toolchain and its greeting in, so this is that shell plus a cluster,
        # never an alternative to it.
        #
        # Nothing here needs the system's nix config. kind runs the control
        # plane and its nodes as containers on the same docker daemon `just up`
        # already uses, so a local cluster costs no kubelet service, no CNI and
        # no k3s unit on the host. The docker caveat above applies unchanged:
        # the daemon is the host's to provide.
        kube = pkgs.mkShell {
          name = "actias-kube";

          inputsFrom = [ self.devShells.${pkgs.stdenv.hostPlatform.system}.default ];

          nativeBuildInputs = with pkgs; [
            kind
            kubectl
            kubernetes-helm

            # R2a gates a chart-touching PR on `ct lint`; the same binary
            # locally means the gate is reproducible before it is pushed.
            chart-testing

            # Rendering the chart without a cluster, which is most of the
            # R1b-R1e loop: template, then check the output is valid against
            # real schemas rather than by eye.
            kubeconform

            # Reading a cluster the way `just logs` reads compose.
            k9s
            stern
          ];

          shellHook = ''
            if [ -t 1 ]; then
              echo "kube: kind $(kind version | cut -d' ' -f2), kubectl $(kubectl version --client=true | head -1 | cut -d' ' -f3), helm $(helm version --short)"
            fi
          '';
        };
      });
    };
}
