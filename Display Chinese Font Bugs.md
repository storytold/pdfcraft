# 中文字體顯示問題修復

> 軟件：PdfCraft 0.5.0（純 Rust 的 PDF 工作台，Adobe Acrobat 開源重寫版）
> 修復日期：2026-10-10
> 平台：Windows 11 Enterprise x64（本機從源碼編譯）
> 編譯工具鏈：rustc/cargo 1.99.0（stable-x86_64-pc-windows-msvc）+ VS 2022 Community MSVC 14.44

---

## 1. 問題現象

應用程序可正常打開、功能可用，但界面中 **所有中文字符無法顯示**（顯示為方塊 □），看起來像是「編碼問題」。

## 2. 根因分析

**這不是編碼問題，而是字體缺失問題。**

PdfCraft 界面（egui）的字體族按以下順序組裝（`crates/ui-egui/src/theme.rs` 的 `font_definitions_for`）：

1. 內嵌字體：Inter、JetBrains Mono（僅覆蓋拉丁/希臘/西里爾文字）
2. egui 默認字體（亦無 CJK 覆蓋）
3. **craft-fonts 的 CJK/阿拉伯/泰盧固字體** ← 官方發行版才嵌入
4. 系統回退字體（`system_fonts::fallback()`）：Windows 上僅嘗試 `segoeui.ttf`、`tahoma.ttf`、`arial.ttf`，且用 **阿拉伯字母 alef (U+0627)** 探測有效性 —— **這些字體均不含中文字形**

官方發行版構建時嵌入 craft-fonts（中日韓字體），因此中文正常。本機直接 `cargo build` **沒有設置 `CRAFT_FONTS_DIR`**，第 3 層為空，第 4 層又不覆蓋 CJK——最終整個字體鏈**沒有任何中文字形**，所有中文（菜單、面板、文件名、標題）都渲染成方塊。

## 3. 修復方案

在不引入 craft-fonts 外部依賴的前提下，為「無內嵌 CJK 字體的構建」增加一層**運行時系統 CJK 字體回退**：

- 新增 `system_fonts::cjk()`：像現有的阿拉伯語回退一樣，在運行時讀取系統已安裝的 CJK 字體（不改寫、不嵌入、不發布，符合項目規則 AGENTS.md §1.4）；
- 僅當 `pdfcraft_fonts::ui_cjk_fonts(prefer_hans)` 為空（即本構建沒有任何內嵌漢字字形）時，才把該系統字體掛入每個字體族末尾 —— **帶 craft-fonts 的官方發行版行為完全不受影響**；
- `PDFCRAFT_SYSTEM_FONTS=0` 環境變量仍可關閉全部系統字體回退。

共修改 **2 個文件**：`crates/ui-egui/src/system_fonts.rs`、`crates/ui-egui/src/theme.rs`。

---

## 4. 代碼修改前後對比

### 4.1 `crates/ui-egui/src/system_fonts.rs`

#### ① 模塊文檔

```diff
- //! The last-resort interface font: one face already installed on this machine.
- //!
- //! The embedded faces (Inter, egui's defaults, craft-fonts) come first in every family; this one
- //! only draws characters none of them has, such as an Arabic file name in a build without
- //! craft-fonts. It is read at runtime and never embedded or shipped (AGENTS.md §1.4), and
- //! `PDFCRAFT_SYSTEM_FONTS=0` turns it off (published screenshots do).
+ //! The last-resort interface fonts: faces already installed on this machine.
+ //!
+ //! The embedded faces (Inter, egui's defaults, craft-fonts) come first in every family; these
+ //! ones only draw characters none of them has: an Arabic file name, or CJK interface text in a
+ //! build without craft-fonts. They are read at runtime and never embedded or shipped
+ //! (AGENTS.md §1.4), and `PDFCRAFT_SYSTEM_FONTS=0` turns them off (published screenshots do).
```

#### ② 新增漢字探測字符

```diff
  /// Arabic letter alef: the face must have it to be worth loading.
  const PROBE: char = '\u{0627}';
+ /// Han character U+4E2D 中: the CJK face must have it to be worth loading.
+ const HAN_PROBE: char = '中';
```

#### ③ 新增 `cjk()` 公開函數與加載邏輯

```diff
  /// The installed fallback face, read once. `None` when it is turned off or no candidate fits.
  pub fn fallback() -> Option<Arc<FontData>> {
      static CACHE: OnceLock<Option<Arc<FontData>>> = OnceLock::new();
      CACHE.get_or_init(load).clone()
  }
 
+ /// The installed Han (CJK) fallback face, read once. `None` when it is turned off or no
+ /// candidate fits; see [`cjk_candidates`].
+ pub fn cjk() -> Option<Arc<FontData>> {
+     static CACHE: OnceLock<Option<Arc<FontData>>> = OnceLock::new();
+     CACHE.get_or_init(load_cjk).clone()
+ }
+
  fn load() -> Option<Arc<FontData>> {
      if std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|v| v == "0") {
          return None;
      }
      candidates().iter().find_map(|path| read(path))
  }
+
+ fn load_cjk() -> Option<Arc<FontData>> {
+     if std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|v| v == "0") {
+         return None;
+     }
+     cjk_candidates().iter().find_map(|path| read_for(path, HAN_PROBE))
+ }
```

#### ④ 新增跨平台 CJK 候選字體列表

```diff
  /// Well-known locations of faces with broad script coverage, best first.
  fn candidates() -> Vec<PathBuf> {
      ...（原阿拉伯語候選，未改動）...
  }
+
+ /// Well-known locations of faces covering Han scripts (Chinese, Japanese), best first. At most
+ /// one is loaded: the first whose file parses and maps [`HAN_PROBE`].
+ fn cjk_candidates() -> Vec<PathBuf> {
+     if cfg!(windows) {
+         let dir = std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot")).map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
+         // Microsoft JhengHei (Traditional), Microsoft YaHei (Simplified; still covers most
+         // Traditional), DengXian (SimSun's successor) and SimSun. All ship with Windows.
+         ["msjh.ttc", "msyh.ttc", "Deng.ttf", "simsun.ttc"].iter().map(|f| dir.join("Fonts").join(f)).collect()
+     } else if cfg!(target_os = "macos") {
+         ["/System/Library/Fonts/PingFang.ttc", "/System/Library/Fonts/STHeiti Medium.ttc", "/System/Library/Fonts/Hiragino Sans GB.ttc"]
+             .iter()
+             .map(PathBuf::from)
+             .collect()
+     } else {
+         let files = [
+             "opentype/noto/NotoSansCJK-Regular.ttc",
+             "truetype/wqy/wqy-microhei.ttc",
+             "truetype/arphic/uming.ttc",
+         ];
+         ["/usr/share/fonts", "/usr/local/share/fonts"].iter().flat_map(|dir| files.iter().map(move |f| Path::new(dir).join(f))).collect()
+     }
+ }
```

#### ⑤ `read` 重構：探測字符參數化

```diff
  fn read(path: &Path) -> Option<Arc<FontData>> {
+     read_for(path, PROBE)
+ }
+
+ fn read_for(path: &Path, probe: char) -> Option<Arc<FontData>> {
      let meta = std::fs::metadata(path).ok()?;
      if !meta.is_file() || meta.len() > MAX_BYTES {
          return None;
      }
      let bytes = std::fs::read(path).ok()?;
-     let index = face_with(&bytes, PROBE)?;
+     let index = face_with(&bytes, probe)?;
      let mut data = FontData::from_owned(bytes);
      data.index = index;
      log::info!("interface font fallback: {}", path.display());
      Some(Arc::new(data))
  }
```

### 4.2 `crates/ui-egui/src/theme.rs`

#### ① 新增字體名常量

```diff
  /// The name of the installed face [`installed_font_definitions`] may add after the embedded ones.
  pub const SYSTEM_FALLBACK: &str = "system-fallback";
+ /// The name of the installed Han face [`installed_font_definitions`] adds when the build has no
+ /// embedded CJK faces (see [`crate::system_fonts::cjk`]) so the Chinese interface still draws.
+ pub const SYSTEM_FALLBACK_CJK: &str = "system-fallback-cjk";
```

#### ② `installed_font_definitions`：掛載 CJK 回退字體

```diff
- /// What [`install_fonts_for`] installs: [`font_definitions_for`], then, on desktop, one face
- /// already installed on this machine as the last fallback of every family. It only draws
- /// characters no embedded face has (an Arabic file name in a build without craft-fonts);
- /// `PDFCRAFT_SYSTEM_FONTS=0` leaves it out.
+ /// What [`install_fonts_for`] installs: [`font_definitions_for`], then, on desktop, up to two
+ /// faces already installed on this machine as the last fallbacks of every family. They only
+ /// draw characters no embedded face has: an Arabic file name, and CJK interface text in a
+ /// build without craft-fonts; `PDFCRAFT_SYSTEM_FONTS=0` leaves them out.
  pub fn installed_font_definitions(prefer_hans: bool) -> FontDefinitions {
      #[cfg_attr(target_arch = "wasm32", expect(unused_mut))]
      let mut fonts = font_definitions_for(prefer_hans);
      #[cfg(not(target_arch = "wasm32"))]
-     if let Some(data) = crate::system_fonts::fallback() {
-         fonts.font_data.insert(SYSTEM_FALLBACK.to_owned(), data);
-         for stack in fonts.families.values_mut() {
-             stack.push(SYSTEM_FALLBACK.to_owned());
-         }
-     }
+     {
+         if let Some(data) = crate::system_fonts::fallback() {
+             fonts.font_data.insert(SYSTEM_FALLBACK.to_owned(), data);
+         }
+         // A build without craft-fonts embeds no Han faces; cover the Chinese interface with
+         // one installed CJK face instead. With craft-fonts the embedded faces come first and
+         // this last face would rarely, if ever, be reached, so it is left out then.
+         if pdfcraft_fonts::ui_cjk_fonts(prefer_hans).is_empty()
+             && let Some(data) = crate::system_fonts::cjk()
+         {
+             fonts.font_data.insert(SYSTEM_FALLBACK_CJK.to_owned(), data);
+         }
+         for stack in fonts.families.values_mut() {
+             if fonts.font_data.contains_key(SYSTEM_FALLBACK) {
+                 stack.push(SYSTEM_FALLBACK.to_owned());
+             }
+             if fonts.font_data.contains_key(SYSTEM_FALLBACK_CJK) {
+                 stack.push(SYSTEM_FALLBACK_CJK.to_owned());
+             }
+         }
+     }
      fonts
  }
```

---

## 5. 設計考量

| 考量 | 處理方式 |
|---|---|
| 不影響官方發行版 | 只有 `ui_cjk_fonts(prefer_hans)` 為空（未嵌入 craft-fonts）的構建才會掛載系統 CJK 字體；發行版嵌入字體在前，此回退幾乎永遠不會被用到，故直接不掛載 |
| 許可合規（AGENTS.md §1.4） | 字體是**運行時讀取系統已安裝文件**（首次啟動時惰性加載並緩存於 `OnceLock`），不嵌入二進製、不隨程序發布 |
| 安全邊界 | 沿用現有防護：單文件 ≤ 32 MiB、最多嘗試 16 個 face、用 `skrifa` 與 egui 相同的解析器預先探測字符映射，路徑視為不可信輸入 |
| 可關閉性 | `PDFCRAFT_SYSTEM_FONTS=0` 同時關閉阿拉伯語與 CJK 兩層回退（官方截圖流程不受影響） |
| 繁簡覆蓋 | Windows 候選排序：微軟正黑體（繁）→ 微軟雅黑（簡，兼顧大部分繁體）→ 等線 → 宋體。`find_map` 取第一個包含「中」字的可用字體，各語言包環境均能命中 |
| 跨平台 | macOS：PingFang / STHeiti / Hiragino Sans GB；Linux：Noto Sans CJK / 文泉驛微米黑 / AR PL UMing |
| wasm32 | 桌面端才掛載（`#[cfg(not(target_arch = "wasm32"))]`），Web 構建保持 25 MiB 托管限制不變 |

## 6. 驗證結果

| 步驟 | 結果 |
|---|---|
| `cargo check --release -p pdfcraft-ui-egui -j 2` | ✅ 通過（8.9s） |
| `cargo build --release -p pdfcraft -j 2` | ✅ 通過（7m45s，0 錯誤） |
| 啟動後日誌（`%APPDATA%\PdfCraft\data\logs\pdfcraft.log`） | ✅ 兩層回退均生效： |
| | `interface font fallback: C:\WINDOWS\Fonts\segoeui.ttf` |
| | `interface font fallback: C:\WINDOWS\Fonts\msjh.ttc`（微軟正黑體，新增） |

## 7. 修改文件清單

| 文件 | 性質 |
|---|---|
| `crates/ui-egui/src/system_fonts.rs` | 新增 CJK 系統字體加載器（`cjk()`、`load_cjk()`、`cjk_candidates()`、`HAN_PROBE`），`read` 重構為探測字符參數化 |
| `crates/ui-egui/src/theme.rs` | 新增 `SYSTEM_FALLBACK_CJK` 常量；`installed_font_definitions` 在無內嵌 CJK 字體時掛載系統回退 |

> 注：另有兩個無關的既有警告（contributors.json / people.toml 路徑提示，源碼目錄從 Downloads 遷移至 `C:\Program Files\pdfcraft-0.5.0` 所致，僅影響「關於」窗口的貢獻者展示，不影響功能）。