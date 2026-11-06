self:
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.programs.pdfcraft;
in
{
  options.programs.pdfcraft = {
    enable = lib.mkEnableOption "PdfCraft, a PDF viewer and editor";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "pdfcraft.packages.\${pkgs.stdenv.hostPlatform.system}.default";
      description = "The PdfCraft package to install.";
    };

    defaultPdfHandler = lib.mkEnableOption "PdfCraft as the default application for PDF files";
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];

    xdg.mimeApps = lib.mkIf cfg.defaultPdfHandler {
      enable = true;
      defaultApplications."application/pdf" = [ "ai.storyteller.pdfcraft.desktop" ];
    };
  };
}
