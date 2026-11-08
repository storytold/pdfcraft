{
  lib,
  rustPlatform,
  fetchFromGitHub,
  fetchurl,
  cmake,
  perl,
  pkg-config,
  makeWrapper,
  wayland,
  libxkbcommon,
  libGL,
  vulkan-loader,
  libx11,
  libxcursor,
  libxi,
  libxrandr,
  cups,
}:

let
  src = lib.cleanSourceWith {
    src = ../.;
    filter =
      path: type:
      !(builtins.elem (baseNameOf path) [
        "target"
        "dist"
        "result"
        "flake.nix"
        "flake.lock"
        "nix"
      ]);
  };

  cargoToml = lib.importTOML ../Cargo.toml;
  attribution = lib.importTOML ../ATTRIBUTION.toml;

  craftI18n = fetchFromGitHub rec {
    name = "craft-i18n-${rev}";
    owner = "storytold";
    repo = "craft-i18n";
    rev = lib.fileContents ../craft-i18n.rev;
    hash = "sha256-KTP2u+/3LHry7fOLUpv+UOXDBU7LZI5+1U9605jXtT4=";
  };

  models = builtins.filter (e: (e.kind or "") == "model") attribution.fetched;

  fetchModel = m: {
    inherit (m) file;
    model = fetchurl {
      inherit (m) url sha256;
    };
    licence = fetchurl {
      url = m.licence_url;
      sha256 = m.licence_sha256;
    };
  };

  installModel = m: ''
    install -Dm644 ${m.model} $out/share/pdfcraft/models/${m.file}
    install -Dm644 ${m.licence} $out/share/pdfcraft/models/${m.file}.LICENCE.txt
  '';

  runtimeLibs = [
    wayland
    libxkbcommon
    libGL
    vulkan-loader
    libx11
    libxcursor
    libxi
    libxrandr
  ];
in
rustPlatform.buildRustPackage {
  pname = "pdfcraft";
  inherit (cargoToml.workspace.package) version;
  inherit src;

  cargoLock = {
    lockFile = ../Cargo.lock;
    allowBuiltinFetchGit = true;
  };

  nativeBuildInputs = [
    cmake
    perl
    pkg-config
    makeWrapper
  ];

  postPatch = ''
    sed -i -E '/^(egui_demo_lib|egui_inspection|egui_system_font_provider) = /d' Cargo.toml
  '';

  dontUseCmakeConfigure = true;

  cargoBuildFlags = [
    "-p"
    "pdfcraft"
    "-p"
    "pdfcraft-cli"
  ];
  doCheck = false;

  env = {
    CRAFT_I18N_DIR = "${craftI18n}";
    CRAFT_I18N_REQUIRED = "1";
    PDFCRAFT_NO_STARTUP_UPDATE_CHECK = "1";
  };

  postInstall = ''
    install -Dm644 packaging/linux/ai.storyteller.pdfcraft.desktop -t $out/share/applications
    install -Dm644 packaging/linux/ai.storyteller.pdfcraft.mime.xml $out/share/mime/packages/ai.storyteller.pdfcraft.xml
    mkdir -p $out/share/icons
    cp -R assets/app-icon/hicolor $out/share/icons/
    ${lib.concatMapStrings (m: installModel (fetchModel m)) models}
  '';

  postFixup = ''
    patchelf --add-rpath ${lib.makeLibraryPath runtimeLibs} $out/bin/pdfcraft
    wrapProgram $out/bin/pdfcraft --suffix PATH : ${lib.getBin cups}/bin
  '';

  meta = {
    description = "Read, organize, combine, split and secure PDFs";
    homepage = "https://github.com/storytold/pdfcraft";
    license = with lib.licenses; [
      mit
      asl20
    ];
    mainProgram = "pdfcraft";
    platforms = lib.platforms.linux;
  };
}
