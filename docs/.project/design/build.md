最终方案我建议这样定：**Orix 作为 CLI 工具，核心目标是安装后能直接在终端执行 `orix`。**

## 一、最终发布矩阵

### Windows

| 产物                              | 是否必须 | 作用                                  |
| --------------------------------- | -------: | ------------------------------------- |
| `orix-x86_64-pc-windows-msvc.zip` |     必须 | 绿色版，开发者手动解压使用            |
| `orix-x86_64-pc-windows-msvc.msi` |     必须 | Windows 标准安装包，自动写入用户 PATH |
| `orix-installer.ps1`              |     推荐 | 一行命令安装，适合开发者              |
| `orix-setup.exe`                  |     暂缓 | 等需要漂亮 UI、复杂安装流程时再做     |

Windows 最推荐：**MSI + ZIP + PowerShell installer**。WiX 本身就是用于生成 Windows Installer 包的工具，MSI 对企业部署、卸载、修复、静默安装更友好；WiX 也支持通过 Environment 配置环境变量。([FireGiant Docs][1])

### macOS

| 产物                               | 是否必须 | 作用                          |
| ---------------------------------- | -------: | ----------------------------- |
| `orix-aarch64-apple-darwin.tar.xz` |     必须 | M 系列 Mac                    |
| `orix-x86_64-apple-darwin.tar.xz`  |     必须 | Intel Mac                     |
| `orix-installer.sh`                |     必须 | 自动安装到用户目录并处理 PATH |
| Homebrew Tap                       | 强烈推荐 | macOS 最舒服的长期安装方式    |
| `.pkg`                             |     可选 | 需要图形安装器/企业分发时再做 |

macOS 最推荐：**Homebrew + tar.xz + shell installer**。如果做 `.pkg`，可以通过 `pkgbuild --scripts` 放入 `postinstall` 脚本创建 `/usr/local/bin/orix` 软链；`pkgbuild` 的 man page 明确支持 `preinstall` / `postinstall` 脚本。([Unix][2])

### Linux

| 产物                                    |              是否必须 | 作用                                |
| --------------------------------------- | --------------------: | ----------------------------------- |
| `orix-x86_64-unknown-linux-gnu.tar.xz`  |                  必须 | 通用 glibc Linux                    |
| `orix-x86_64-unknown-linux-musl.tar.xz` |                  推荐 | 更通用的静态版本                    |
| `orix-aarch64-unknown-linux-gnu.tar.xz` |                  推荐 | ARM64 Linux                         |
| `orix-installer.sh`                     |                  必须 | 自动安装到 `~/.local/bin`           |
| `.deb`                                  |                  推荐 | Ubuntu / Debian 用户体验更好        |
| `.rpm`                                  |                  后续 | Fedora / RHEL / openSUSE            |
| AppImage                                | 不推荐作为 CLI 主方案 | 更适合桌面应用，不适合终端命令 PATH |

Linux 包安装 CLI 时，二进制通常应进入 `/usr/bin` 这类 PATH 目录；Debian Policy 也明确限制不要往 `/bin/*` 等 usr-merged 目录直接安装文件。([Debian][3])

---

## 二、最终推荐安装路径

### Windows

默认安装到当前用户目录，避免管理员权限：

```txt
%LOCALAPPDATA%\Programs\Orix\bin\orix.exe
```

然后把这个目录加入 **用户 PATH**：

```txt
%LOCALAPPDATA%\Programs\Orix\bin
```

安装后：

```powershell
orix --version
```

卸载时：

```txt
删除安装目录
从用户 PATH 中移除 %LOCALAPPDATA%\Programs\Orix\bin
```

不推荐默认写系统 PATH，因为系统 PATH 通常需要管理员权限，而且风险更高。

---

### macOS

Homebrew 安装时不需要自己处理 PATH：

```bash
brew install baicie/tap/orix
```

Homebrew 会把命令链接到 Homebrew 的 bin 目录。

手动安装脚本则推荐：

```txt
~/.orix/bin/orix
```

并确保 shell 配置中有：

```bash
export PATH="$HOME/.orix/bin:$PATH"
```

如果做 `.pkg`，可以安装到：

```txt
/usr/local/orix/bin/orix
```

然后创建软链：

```bash
/usr/local/bin/orix -> /usr/local/orix/bin/orix
```

---

### Linux

用户级安装：

```txt
~/.local/bin/orix
```

并确保：

```bash
export PATH="$HOME/.local/bin:$PATH"
```

deb/rpm 系统级安装：

```txt
/usr/bin/orix
```

安装后直接：

```bash
orix --version
```

---

## 三、最终 Release 文件命名

以 `v0.1.0` 为例：

```txt
orix-v0.1.0-x86_64-pc-windows-msvc.zip
orix-v0.1.0-x86_64-pc-windows-msvc.msi
orix-v0.1.0-aarch64-apple-darwin.tar.xz
orix-v0.1.0-x86_64-apple-darwin.tar.xz
orix-v0.1.0-x86_64-unknown-linux-gnu.tar.xz
orix-v0.1.0-x86_64-unknown-linux-musl.tar.xz
orix-v0.1.0-aarch64-unknown-linux-gnu.tar.xz
orix-installer.sh
orix-installer.ps1
SHA256SUMS
```

后续再加：

```txt
orix-v0.1.0-amd64.deb
orix-v0.1.0-x86_64.rpm
orix-v0.1.0-setup.exe
```

---

## 四、最终安装命令设计

### Windows

推荐给用户：

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/baicie/orix/releases/latest/download/orix-installer.ps1 | iex"
```

或者：

```powershell
msiexec /i orix-v0.1.0-x86_64-pc-windows-msvc.msi
```

静默安装：

```powershell
msiexec /i orix-v0.1.0-x86_64-pc-windows-msvc.msi /qn
```

验证：

```powershell
orix --version
```

---

### macOS / Linux

推荐：

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/baicie/orix/releases/latest/download/orix-installer.sh | sh
```

验证：

```bash
orix --version
```

macOS 后续推荐：

```bash
brew install baicie/tap/orix
```

Ubuntu/Debian 后续推荐：

```bash
sudo dpkg -i orix-v0.1.0-amd64.deb
```

---

## 五、打包工具选择

### 第一阶段：最推荐用 `cargo-dist`

你的项目如果是 Rust CLI，**优先用 cargo-dist**。它的定位就是帮 Rust/CLI 项目完成构建产物、安装器、Release 分发这些事情，官方文档也把它拆成 build 和 distribute 两部分。([Axodotdev][4])

它现在已经提供 shell installer、PowerShell installer、Homebrew、npm 包装安装等路径；官方 Release 页也展示了这些安装方式。([GitHub][5])

初始化：

```bash
cargo install cargo-dist
cargo dist init
```

选择目标：

```txt
x86_64-pc-windows-msvc
aarch64-apple-darwin
x86_64-apple-darwin
x86_64-unknown-linux-gnu
x86_64-unknown-linux-musl
aarch64-unknown-linux-gnu
```

选择安装器：

```txt
shell
powershell
homebrew
npm 可选
msi 可选
```

然后它会生成 GitHub Actions release workflow。

---

## 六、推荐项目配置

`Cargo.toml` 里至少保证：

```toml
[package]
name = "orix"
version = "0.1.0"
edition = "2021"
description = "A fast modern CLI tool"
license = "MIT"
repository = "https://github.com/baicie/orix"

[[bin]]
name = "orix"
path = "src/main.rs"

[profile.release]
lto = true
codegen-units = 1
strip = true
opt-level = "z"

[profile.dist]
inherits = "release"
lto = true
codegen-units = 1
strip = true
opt-level = "z"
```

---

## 七、Windows PATH 策略

### MSI 默认行为

安装：

```txt
%LOCALAPPDATA%\Programs\Orix\bin\orix.exe
```

写入用户 PATH：

```txt
%LOCALAPPDATA%\Programs\Orix\bin
```

卸载时移除 PATH。

### EXE 后续行为

如果后续做 `orix-setup.exe`，它本质是安装器，可以做：

```txt
1. 检测当前系统架构
2. 选择安装目录
3. 写入用户 PATH
4. 创建卸载项
5. 卸载时清理 PATH
6. 可选安装 shell completion
```

但是第一阶段没必要优先做 EXE。

**结论：Windows 先 MSI，不要一开始就卷 EXE。**

---

## 八、macOS / Linux PATH 策略

### `installer.sh` 逻辑

推荐安装到用户目录：

```bash
~/.orix/bin/orix
```

然后检查这些配置文件：

```txt
~/.zshrc
~/.bashrc
~/.bash_profile
~/.profile
```

如果没有 PATH，则追加：

```bash
export PATH="$HOME/.orix/bin:$PATH"
```

安装脚本最后提示：

```bash
source ~/.zshrc
# 或重新打开终端
```

### 为什么不用默认写 `/usr/local/bin`

因为 `/usr/local/bin` 通常需要 sudo，不适合一行安装脚本。用户级安装更安全。

### Homebrew / deb / rpm 则不需要自己改 shell 配置

这些包管理器本来就会把命令放到系统 PATH 目录里。

---

## 九、AppImage 的最终定位

AppImage 不作为 CLI 主安装包。

如果你后续做 Orix 桌面应用，例如 GUI 工具，那么 Linux 可以提供：

```txt
orix-v0.1.0-x86_64.AppImage
```

但是 CLI 仍然应该走：

```txt
tar.xz
installer.sh
deb
rpm
```

你之前遇到的 AppImage 报错：

```txt
More than one architectures were found of the AppDir source directory "AppDir"
A valid architecture with the ARCH environmental variable should be provided
```

如果继续打 AppImage，需要显式指定：

```bash
ARCH=x86_64 appimagetool AppDir orix-x86_64.AppImage
```

但这只是修 AppImage 的问题，不影响 CLI 主方案。

---

## 十、最终 CI 流程

发布流程：

```txt
1. 修改 Cargo.toml version
2. 更新 CHANGELOG.md
3. git commit
4. git tag v0.1.0
5. git push origin main --tags
6. GitHub Actions 自动构建多平台产物
7. 自动创建 GitHub Release
8. 上传 zip/tar/msi/installers/SHA256SUMS
```

本地发布前检查：

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo build --release
cargo dist plan
```

正式发版：

```bash
git tag v0.1.0
git push origin v0.1.0
```

---

## 十一、最终优先级

### v0.1.0 必做

```txt
Windows zip
Windows msi
macOS arm64 tar.xz
macOS x64 tar.xz
Linux x64 gnu tar.xz
Linux x64 musl tar.xz
installer.sh
installer.ps1
SHA256SUMS
GitHub Release 自动发布
```

### v0.2.0 推荐做

```txt
Homebrew Tap
Linux deb
shell completions
orix self update 可选
```

### v0.3.0 再做

```txt
rpm
npm wrapper: npm i -g @baicie/orix
Windows setup.exe
macOS pkg
```

### 不建议早期做

```txt
AppImage 作为 CLI 主安装方式
Snap
Flatpak
复杂 GUI 安装器
自动 node 版本管理
自动换源
```

---

## 十二、最终一句话方案

**Orix 第一阶段就按“专业 CLI 工具”做：**

```txt
Windows：MSI + ZIP + PowerShell installer，MSI 自动写用户 PATH
macOS：tar.xz + shell installer + Homebrew Tap，优先 Homebrew
Linux：tar.xz + shell installer + deb，二进制进入 ~/.local/bin 或 /usr/bin
AppImage：只给未来 GUI 桌面版，不作为 CLI 主安装方式
```

这样最稳，维护成本最低，用户安装后也能直接：

```bash
orix --version
```

[1]: https://docs.firegiant.com/wix3/main/?utm_source=chatgpt.com "Introduction to WiX toolset - FireGiant Docs"
[2]: https://www.unix.com/man_page/osx/1/pkgbuild/?utm_source=chatgpt.com "pkgbuild(1) osx man page | unix.com"
[3]: https://www.debian.org/doc/debian-policy/ch-files.html?utm_source=chatgpt.com "10. Files — Debian Policy Manual v4.7.4.1"
[4]: https://axodotdev.github.io/cargo-dist/book/introduction.html?utm_source=chatgpt.com "Introduction - dist"
[5]: https://github.com/axodotdev/cargo-dist/releases?utm_source=chatgpt.com "Releases · axodotdev/cargo-dist"
