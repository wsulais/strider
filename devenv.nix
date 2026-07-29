{ pkgs, ... }: {
  # The wasm target is not optional: GUARD-PORTABILITY-TARGET-COMPILES
  # ([[RFC-0003:C-PORT-GATE]]) compiles every library crate for a host with no
  # filesystem, no threads and no blocking I/O. Cross-compiling requires the
  # stable channel — the nixpkgs channel cannot add targets.
  languages.rust = {
    enable = true;
    channel = "stable";
    targets = [ "wasm32-unknown-unknown" ];
  };

  packages = [
    pkgs.cargo-deny  # GUARD-LIBRARY-LICENCE-COMPATIBILITY ([[RFC-0002:C-LICENSE]] 4)
    pkgs.reuse       # SPDX/REUSE conformance ([[RFC-0002:C-LICENSE]] 3)
    pkgs.jq
  ];
}
