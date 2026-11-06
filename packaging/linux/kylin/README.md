# Kylin / UOS deb packages (glibc 2.31 baseline)

`packaging/linux/package.sh` builds the regular distro packages on Ubuntu 22.04, so its deb
declares `libc6 (>= 2.35)` — which **麒麟 V20 / 优麒麟** (Ubuntu 20.04 base, glibc 2.31-0kylin),
UOS/deepin desktops and Debian 11 cannot provide: the binary fails with `GLIBC_2.34' not found`
(or dpkg refuses the dependency) no matter how many libraries are installed.

This directory builds a second, older-baseline pair of debs inside a **Debian bullseye**
(glibc 2.31) container, following the deepin-screenshot packaging approach:

    pdfcraft-<version>-kylin-<osarch>.deb          regular package
    pdfcraft-<version>-kylin-<osarch>-offline.deb  + bundled runtime dependency closure

`<osarch>` is `amd64` or `aarch64`. The package version is `<version>+kylin` (sorts after
`<version>`, before `<version>+1`), and the build carries the `kylin` label, so `--version` and
About read `0.7.1 (kylin)` and it can't be mistaken for a regular build.

## 安装（offline install）

两个 deb 变体，按需二选一：

- `pdfcraft-<版本>-kylin-<架构>.deb` — 常规包。Depends 只声明真正的 ELF 依赖
  （`libc6 (>= 2.31)`、`libgcc-s1`）；X11/Wayland、xkbcommon、Vulkan/EGL/GL 在运行时加载，
  桌面系统都自带。适合已经能跑 GUI 程序的麒麟 V20 / UOS / deepin 桌面。

      sudo dpkg -i pdfcraft-<版本>-kylin-<架构>.deb

- `pdfcraft-<版本>-kylin-<架构>-offline.deb` — 离线包。在常规内容之外打包了运行时依赖闭包
  （`/usr/share/pdfcraft/deps/*.deb`）和安装脚本，并包含 OCR 模型，适合无外网的机器。

      sudo dpkg -i pdfcraft-<版本>-kylin-<架构>-offline.deb

安装结束时 postinst 会用 `ldd` 检测缺失的运行库：**全部齐备则无事发生；有缺失则自动通过
systemd 延迟 10 秒执行依赖安装**（等 dpkg 锁释放，结果在 `journalctl -u pdfcraft-deps`）。
systemd 不可用时回退为提示手动执行：

    sudo /usr/share/pdfcraft/install-deps.sh

脚本逻辑（版本感知）：逐个检查 `deps/` 里的依赖包，系统已安装且版本不低的直接用系统的；
系统缺失或更旧的，用包内副本安装/升级（合并成一次 `dpkg -i` 调用，由 dpkg 按依赖顺序配置）。
`deps/` 是构建时按 Depends 递归展开的完整闭包——麒麟 V20 与 Debian 对部分库的包名不同
（如 libicu66 对 libicu63），不同名的库可以共存，逐层展开才能保证离线 `dpkg -i` 不缺依赖。
执行完再做一次 `ldd` 校验，报告 `OK: all runtime libraries resolved.` 或仍缺失的库。

### 两个硬边界

- **glibc 不能打包替换**。所有产物在 bullseye 容器（glibc 2.31）构建，要求目标机
  glibc ≥ 2.31：麒麟 V20 / 优麒麟 ✓、UOS/deepin 桌面 ✓、Debian 11+ ✓。更老的系统
  （如 UOS 20，glibc 2.28）不在支持范围，需要目标机自编译。
- **维护脚本里不能嵌套 `dpkg -i`**。dpkg 全程持锁，postinst 只做检测，通过 systemd 延迟执行
  安装脚本（等同手动执行），systemd 不可用时提示手动运行。

## 构建与发布

CI（`.github/workflows/kylin-deb.yml`）在原生 runner（amd64 + aarch64）的
`debian:bullseye-slim` 容器里跑 `packaging/linux/kylin/package.sh`：装 bullseye 的构建依赖和
rustup（bullseye 自带的 rustc 1.63 低于本仓库 `rust-version`），克隆 craft-fonts（与
release.yml 同一 pinned ref），`cargo build --release --locked`，再打出两个 deb。推送 main
自动构建（与 deepin-screenshot 的 master 一致），任何分支可手动触发；推 `v*` tag 时把 deb
附到该 tag 的 Release 上。

本机有 Docker 时的等价做法（ amd64 示例）：

    docker run --rm -v "$PWD":/src -w /src debian:bullseye-slim bash -lc '
      sed -i "s|http://security.debian.org|http://archive.debian.org|g; s|http://deb.debian.org/debian-security|http://archive.debian.org/debian-security|g" /etc/apt/sources.list
      echo "Acquire::Check-Valid-Until \"false\";" > /etc/apt/apt.conf.d/99no-valid-until
      apt-get update
      apt-get install -y --no-install-recommends build-essential cmake perl pkg-config git curl ca-certificates nasm dpkg-dev
      curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
      . "$HOME/.cargo/env"
      git init -q /tmp/craft-fonts && git -C /tmp/craft-fonts remote add origin https://github.com/storytold/craft-fonts.git
      git -C /tmp/craft-fonts fetch --depth 1 origin 3d9ab3a132ed0eff457e424226a435a901da30d3
      git -C /tmp/craft-fonts checkout --detach FETCH_HEAD
      export CRAFT_FONTS_DIR=/tmp/craft-fonts
      packaging/linux/kylin/package.sh'

产物在 `dist/release/`。`--skip-build` 跳过编译、只重新打包。

## 与常规 distro 包（packaging/linux/package.sh）的差异

- glibc 基线 2.31（bullseye 容器），而不是 2.35（Ubuntu 22.04 runner）；
- `dpkg-deb` 直接打包，不用 nfpm：不声明 X11/Vulkan/CUPS 等 Depends（目标发行版包名差异太大），
  离线变体改为打包闭包；
- 文件名带 `-kylin-<osarch>`，包版本带 `+kylin`，构建带 `kylin` label；
- AppImage / rpm / CLI tar 包不做（那些受众是较新发行版，用常规包）；
- staged 目录结构与 package.sh 一致（desktop、mime、metainfo、icons、docs、OCR models），
  两边刻意保持独立：它们服务不同的 glibc 基线，演化节奏不同。

## 踩坑记录

通用部分（容器基线、bullseye 的“半归档”apt 源、递归闭包、postinst 持锁、ci-logs 排查）与
deepin-screenshot 的 README「构建踩坑记录」一致，不重复。本仓库特有的：

1. **bullseye 的 rustc 不能用**。系统 rustc 是 1.63，低于 `rust-version = 1.95`，必须装
   rustup（其官方二进制对 glibc 2.17 即可运行）。
2. **workspace 有 git 依赖**（craft-libs，pinned rev），容器里必须有 `git`；craft-fonts 用
   `git fetch --depth 1 origin <sha>` 取 pinned ref，与 release.yml 一致。
3. **aws-lc-sys 在容器里从源码编 C**（签名验证的 RSA 私钥操作）：需要 `cmake`、`perl`、
   `build-essential`；amd64/arm64 用它自带的预生成绑定，不需要 libclang。若未来启用 bindgen
   再补 `libclang-dev`。
4. **离线闭包很小**：两个二进制只直接链 glibc 和 libgcc_s，其余（X11/Wayland/Vulkan）都是
   运行时 dlopen，闭包只有个位数个包——与 deepin-screenshot（Qt/DTK 全家桶，75~108MB）不同，
   这是 egui 应用的天然优势。
