<p align="center"><img src="docs/plus-banner.svg" alt="eDirStat Plus — folder maps and deeper storage insights" width="100%"></p>

<p align="center">
  <a href="https://github.com/FASTDROPX/edirstat-plus/releases/latest"><img alt="Windows x64" src="https://img.shields.io/badge/download-Windows_x64-3d91c9?style=flat-square"></a>
  <img alt="Rust" src="https://img.shields.io/badge/built_with-Rust-ce9161?style=flat-square">
  <a href="LICENSE"><img alt="MIT license" src="https://img.shields.io/badge/license-MIT-39b8b4?style=flat-square"></a>
</p>

<p align="center"><b>See where your space goes — from whole folders down to individual files.</b><br>Неофициальная доработка eDirStat: карта папок, новые режимы и подробная статистика.</p>

## v2 · More detail, smoother navigation

**[v2 guide / Инструкция →](docs/V2.md)**

- Explicit **folder / file** modes; dense folders no longer collapse at the old 16 pixels²-per-file threshold.
- **Ctrl + wheel** zooms toward the cursor; drag to pan. Finer tiles fade in as you approach.
- **5 palettes** and **9 tile styles**, including vivid WizTree-inspired colors, pastel, glass, neon, and outlines.
- Map context menu: **Show in Explorer** and **Delete…**, using the existing deletion pipeline.
- Corrected MFT counters and scan-start timing; the status bar distinguishes indexing throughput from metadata reads.
- Adaptive partial tree publication, row fades, and a switch to disable live updates.
- Bounded administrator dialog with error reporting and an opt-in **automatically request administrator rights** preference.

Small warm-cache benchmark: 6000 files, median 8.46 ms before / 7.63 ms after. This is not a full-drive or elevated-MFT speed guarantee. UAC and destructive actions were not invoked in testing.

## What's new / Что нового

| Feature | Details |
| --- | --- |
| **Folder maps** | WizTree-inspired nested borders and folder headers, with space reserved for readable labels. |
| **Three layouts** | Folder groups, classic proportional treemap, and alternating stripes. |
| **Four color modes** | File extension, parent folder, size, or modification age. Includes an additional flat shading style. |
| **200+ metrics** | Depth, largest files and folders, percentiles, size and age distributions, file categories, name and path lengths, and special entry flags. |
| **Useful reporting** | Background calculation on Windows, search, section filters, refresh, and copy as TSV for Excel. |
| **Remembered settings** | Layout and color preferences survive restarts and new scans. |

## Download & run

**[Download the Windows x64 build →](https://github.com/FASTDROPX/edirstat-plus/releases/latest)**

Run `edirstat-plus-v2.exe`. Microsoft Visual C++ Runtime x64 must be available.
No installer is required. This community build is not an official upstream release.

## Quick start / Как пользоваться

1. Scan a disk or open a saved snapshot / отсканируйте диск или откройте снимок.
2. Above the map, select **Папки · WizTree**, **Мозаика**, or **Полосы**.
3. Choose **Расширение**, **Папка**, **Размер**, or **Возраст** for coloring.
4. Click **Статистика диска** to explore metrics, search, and copy a report.

Blue → red means smaller → larger or newer → older. Grey in age mode means an unknown modification date. Tiny folders may collapse into a single block to keep the map usable.

## Build from source

Requires stable Rust and the platform build tools. On Windows, install the Visual Studio C++ build tools and Windows SDK.

```powershell
git clone https://github.com/FASTDROPX/edirstat-plus.git
cd edirstat-plus
cargo build --release
.\target\release\edirstat.exe
```

The desktop build uses the default features and Release profile of the original project. The standalone output is `target/release/edirstat.exe` on Windows. A manual Windows build is also available under **Actions → Windows release build**.

## Verification & limits

- Built locally with Rust 1.100.0 for `x86_64-pc-windows-msvc`; PE32+ AMD64 verified.
- Version and headless scanning tested. **118 core + 149 GUI + 11 coordinator tests passed**; one existing symlink test was excluded after Windows returned `WinError 1314` (missing symlink privilege).
- The folder map was inspected in the running app. Statistics rendering and filtering were tested without a native window. The final toolbar spacing adjustment was compiled but not visually rechecked.
- Metrics describe the **scanned snapshot**. A folder-only scan is not a full-disk inventory. Live volume capacity/free-space values are shown separately from saved snapshot data.
- SMART, temperature, fragmentation, and exact duplicate waste are not computed by this new window. Use the existing deduplicator for duplicate analysis.

## Credits & license

Based on **[xangelix/eDirStat](https://github.com/xangelix/edirstat)** by **Cody Neiman**, source baseline [`6afeba03`](https://github.com/xangelix/edirstat/commit/6afeba03a4abc8b6a6c89627a1ef71e752459c14). This repository publishes a modified source snapshot, rather than mirroring upstream history. Original copyright, third-party notices, and **[MIT license](LICENSE)** are retained.

WizTree is referenced as visual inspiration only; this project is not affiliated with WizTree. The new layout is not a pixel-perfect reproduction of its renderer.

Read the **[original README](README.upstream.md)** for the upstream architecture, features, and project support links.
