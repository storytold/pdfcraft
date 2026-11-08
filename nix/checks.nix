{ pkgs, self }:

let
  inherit (pkgs) lib;

  desktopId = "ai.storyteller.pdfcraft.desktop";

  homeManagerStub = {
    options = {
      home.packages = lib.mkOption {
        type = lib.types.listOf lib.types.package;
        default = [ ];
      };
      xdg.mimeApps = {
        enable = lib.mkEnableOption "mimeapps.list";
        defaultApplications = lib.mkOption {
          type = lib.types.attrsOf (lib.types.listOf lib.types.str);
          default = { };
        };
      };
    };
  };

  configWith =
    settings:
    (lib.evalModules {
      modules = [
        homeManagerStub
        (import ./hm-module.nix self)
        { programs.pdfcraft = settings; }
      ];
      specialArgs = { inherit pkgs; };
    }).config;

  packageNames = config: map lib.getName config.home.packages;

  disabled = configWith { };
  installed = configWith { enable = true; };
  handler = configWith {
    enable = true;
    defaultPdfHandler = true;
  };

  expectations = {
    "desktop entry ${desktopId} exists" = builtins.pathExists ../packaging/linux/${desktopId};
    "disabled installs nothing" = disabled.home.packages == [ ];
    "enable installs pdfcraft" = packageNames installed == [ "pdfcraft" ];
    "enable alone leaves mimeapps alone" =
      !installed.xdg.mimeApps.enable && installed.xdg.mimeApps.defaultApplications == { };
    "defaultPdfHandler installs pdfcraft" = packageNames handler == [ "pdfcraft" ];
    "defaultPdfHandler enables mimeapps" = handler.xdg.mimeApps.enable;
    "defaultPdfHandler maps application/pdf" =
      handler.xdg.mimeApps.defaultApplications == { "application/pdf" = [ desktopId ]; };
  };

  package = self.packages.${pkgs.stdenv.hostPlatform.system}.default;

  packageExpectations = {
    "built with PDFCRAFT_UPDATES_MANAGED_EXTERNALLY" = (package.PDFCRAFT_UPDATES_MANAGED_EXTERNALLY or null) == "1";
    "built with the pinned translations" = (package.CRAFT_I18N_REQUIRED or null) == "1";
  };

  checkThat =
    name: expected:
    let
      failed = lib.attrNames (lib.filterAttrs (_: ok: !ok) expected);
    in
    assert lib.assertMsg (failed == [ ]) "${name}: failed: ${lib.concatStringsSep ", " failed}";
    pkgs.runCommand "pdfcraft-${name}-check" { } "touch $out";
in
{
  hm-module = checkThat "hm-module" expectations;
  package-settings = checkThat "package-settings" packageExpectations;
}
