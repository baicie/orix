pnpm 的性能不是靠某一个“快算法”，而是**每个阶段都尽量少做事、复用已有结果、把真实文件数量压到最低**。

可以按这个流水线理解：

```txt
read config / lockfile
  ↓
resolve dependency graph
  ↓
fetch metadata / tarball
  ↓
store content-addressable files
  ↓
import package to virtual store
  ↓
link dependency graph
  ↓
create .bin
  ↓
run lifecycle scripts
  ↓
write lockfile / modules metadata
```

---

# 1. Lockfile 阶段：能不 resolve 就不 resolve

pnpm 默认有 `preferFrozenLockfile=true`。当现有 `pnpm-lock.yaml` 已经满足 `package.json` 的依赖声明时，会走 headless install，跳过依赖解析，因为不需要修改 lockfile。([pnpm][1])

这对 Orix 很关键。

你现在可以做：

```txt
package.json hash 没变
lockfile 满足 manifest
registry/config 没变
graph hash 没变
```

直接进入：

```txt
fetch missing packages
link if layout invalid
```

不要重新 resolve。

对应 Orix 设计：

```rust
if lockfile.validate(&manifest).is_ok() && !opts.force {
    graph = resolve_from_lockfile(lockfile);
    skip_resolver = true;
}
```

---

# 2. Resolve 阶段：缓存元数据 + 减少重复请求

pnpm 在 resolve 阶段核心优化是：

```txt
1. lockfile 可用时跳过 resolve
2. registry metadata 缓存
3. 并发请求 registry
4. workspace / overrides / packageExtensions 提前处理
5. peer 依赖结果可复用
```

pnpm 的配置里还有 `overrides` 和 `packageExtensions`。`overrides` 可以强制改依赖图，甚至移除某些 transitive 依赖；`packageExtensions` 可以修复生态里不完整的依赖声明。这个本质上能减少错误解析、重复回退、无效依赖安装。([pnpm][1])

Orix 这里建议：

```txt
P0:
  metadata cache: package-name -> metadata json
  lockfile fast path

P1:
  resolve task dedupe: 同一个 name@range 同时只请求一次
  workspace package 先建索引
  peer 结果缓存

P2:
  overrides / packageExtensions
```

---

# 3. Fetch 阶段：并发 + retry + 只拉缺失包

pnpm 的网络请求有 `networkConcurrency`，v10.24.0 之后默认会根据 workers 自动选择 16 到 64 之间的并发值；同时有 `fetchRetries`、retry factor、min/max timeout、fetch timeout 等配置。([pnpm][1])

pnpm 还有 `pnpm fetch`：它可以只根据 lockfile 把包预拉到 virtual store，之后 `pnpm install --offline` 就不需要访问 registry。这个对 Docker/CI 很重要，因为 lockfile 不变时可以复用镜像层缓存。([pnpm][2])

Orix 对应优化：

```txt
1. fetch_all 只 fetch store 缺失的包
2. tarball cache 原子写：tmp -> rename
3. extract 失败时删除 cache 并重试一次
4. 同一个 tarball URL 只下载一次
5. 并发值按 CPU/网络自动选择，而不是固定 10
6. 支持 oi fetch + oi i --offline
```

推荐策略：

```rust
let network_concurrency = clamp(num_cpus * 3, 16, 64);
let extract_concurrency = clamp(num_cpus, 4, 16);
```

但注意：**下载并发和解压并发最好分开**。下载可以高一点，解压/写磁盘太高会把磁盘打爆，尤其 Windows。

---

# 4. Store 阶段：内容寻址，避免重复文件

pnpm 的核心优势之一是 content-addressable store。官方文档说明，`node_modules` 里的每个包文件都是指向内容寻址 store 的 hard link。([pnpm][3])

这意味着：

```txt
相同文件内容只存一份
不同项目可以共享 store
node_modules 只是引用/链接
重复安装不用重新下载/解压/复制大量文件
```

Orix 这里应该坚持：

```txt
~/.orix/store/v1/files/sha512/xx/...
~/.orix/store/v1/packages/<name>@<version>/integrity.json
```

关键优化：

```txt
1. import_package 时按 hash 写 content-addressed file
2. 文件已存在则跳过写入
3. integrity.json 记录文件列表
4. package.json 最后写，作为完成标记
5. store import 要原子化，避免半成品
```

你之前 macOS 的 tarball extract 问题，就应该靠：

```txt
tarball cache 原子写
extract 失败 invalidate cache
package import 原子完成标记
```

解决。

---

# 5. Import 阶段：auto import method

pnpm 的 `packageImportMethod` 默认是 `auto`，支持 `auto / hardlink / copy / clone / clone-or-copy`。`auto` 会先尝试 clone；不支持 clone 时尝试 hardlink；clone/hardlink 都不行时 fallback copy。官方文档也说明 clone 是最快且最安全的方式，但不是所有文件系统都支持 CoW clone。([pnpm][1])

这块对 Orix 最重要。

你现在不能只做：

```rust
hard_link(src, dest).or_else(copy)
```

而应该做成“本次安装记忆型 fallback”。

推荐：

```txt
macOS APFS:
  优先 clonefile / copy-on-write
  失败 -> hardlink
  再失败 -> copy

Linux:
  优先 reflink ioctl/FICLONE
  失败 -> hardlink
  再失败 -> copy

Windows:
  优先 hardlink
  跨盘/权限失败 -> copy
```

关键点是：**一旦发现当前 store -> project 跨盘，后续不要每个文件都 hardlink 失败一次，直接切 copy**。

伪代码：

```rust
enum ImportMethod {
    Auto,
    Clone,
    Hardlink,
    Copy,
    CloneOrCopy,
}

enum ResolvedMethod {
    Clone,
    Hardlink,
    Copy,
}

struct PackageImporter {
    method: ImportMethod,
    resolved: Option<ResolvedMethod>,
}

impl PackageImporter {
    fn import_file(&mut self, src: &Path, dest: &Path) -> Result<Outcome> {
        match self.resolved {
            Some(ResolvedMethod::Copy) => return copy(src, dest),
            Some(ResolvedMethod::Hardlink) => return hardlink_or_copy(src, dest),
            Some(ResolvedMethod::Clone) => return clone_or_hardlink_or_copy(src, dest),
            None => {}
        }

        match try_clone(src, dest) {
            Ok(x) => {
                self.resolved = Some(ResolvedMethod::Clone);
                return Ok(x);
            }
            Err(_) => {}
        }

        match try_hardlink(src, dest) {
            Ok(x) => {
                self.resolved = Some(ResolvedMethod::Hardlink);
                return Ok(x);
            }
            Err(e) if is_cross_device_or_permission(&e) => {
                self.resolved = Some(ResolvedMethod::Copy);
                return copy(src, dest);
            }
            Err(e) => return Err(e.into()),
        }
    }
}
```

---

# 6. Virtual store 阶段：固定深度，减少路径爆炸

pnpm 的 `node_modules` 结构不是传统递归嵌套，而是把包放进 `node_modules/.pnpm`，再用 symlink 搭出依赖图。官方文档里说明：真实文件 hard link 到 `.pnpm/<pkg>@<version>/node_modules/<pkg>`，依赖关系再通过 symlink 建出来；不管依赖图多深，目录深度仍然保持稳定。([pnpm][3])

这个优化非常关键：

```txt
避免 npm 老式深层 node_modules 爆路径
减少重复包副本
依赖关系可表达
Node resolution 仍然兼容
```

Orix 现在的 `.orix/<pkg>@<version>/node_modules/<pkg>` 方向是对的，但要补几个优化：

```txt
1. virtual store 目录名做长度限制，Windows 尤其重要
2. package key 超长时 hash 化
3. graph hash 不变时跳过整个 link
4. package import 完成时写 marker
5. package 已存在且 package.json 完整时跳过 import
```

pnpm 也有 `virtualStoreDir` 和 `virtualStoreDirMaxLength`，其中 Windows 默认 max length 更短，并且文档明确说可以把 virtual store 放到盘根目录来缓解 Windows 长路径问题。([pnpm][1])

---

# 7. Link 阶段：少建文件，多建链接，能跳过就跳过

pnpm link 阶段的核心是：

```txt
真实文件：hardlink/clone/copy 到 virtual store
依赖关系：symlink
根依赖：symlink 到 root node_modules
```

官方文档里的流程也很清楚：文件先进 `.pnpm`，然后再 symlink dependencies，最后 direct dependencies symlink 到根 `node_modules`。([pnpm][3])

Orix 这里要重点优化你现在 Windows 卡的问题。

优先级：

```txt
P0：不要每次 unlink 整个 node_modules
P0：graph hash + layout marker 有效时直接跳过 link
P1：只 link changed packages
P1：用 integrity 文件列表，不要 WalkDir 扫 store
P1：package 已完整导入则跳过文件 import
P1：bin shim 已存在且内容一致则不重写
P2：Windows junction/symlink 数量减少
```

你的 link 阶段应该改成：

```txt
1. read .orix-state.json
2. compare graph_hash / linker_version / import_method / platform
3. if valid:
     skip link
4. else:
     diff old graph vs new graph
5. remove only removed packages
6. import only missing packages
7. relink only changed dependency edges
8. update marker atomically
```

marker 示例：

```json
{
  "version": 1,
  "linker": "isolated",
  "graphHash": "abc123",
  "storePath": "C:/Users/me/.orix/store/v1",
  "platform": "win32",
  "arch": "x64",
  "packageImportMethod": "hardlink",
  "createdAt": 1779251654
}
```

---

# 8. Global virtual store：暖缓存下更快

pnpm v10.12.1 引入了 global virtual store 相关能力。文档说明，启用后 `node_modules` 只包含指向 central virtual store 的 symlink；central virtual store 中每个包目录名来自 dependency graph hash，这样多个项目可以共享这些隔离目录。文档也明确说，warm cache 下 global virtual store 可以显著加速安装，但 CI 没缓存时可能变慢。([pnpm][1])

Orix 可以后期做：

```txt
~/.orix/store/v1/links/<graph-hash>/<pkg-key>/node_modules/<pkg>
```

项目里：

```txt
node_modules/.orix -> ~/.orix/store/v1/links/<graph-hash>
node_modules/react -> .orix/react@18.2.0/node_modules/react
```

这个对大型 monorepo 和多项目复用很有价值。

但我建议 Orix 分阶段：

```txt
MVP:
  project local virtual store: node_modules/.orix

P1:
  layout marker + skip link

P2:
  global virtual store
```

---

# 9. Bin 阶段：只写必要 shim，Unix chmod +x

pnpm/npx/npm 这类工具都会在 `.bin` 做平台适配。

性能优化点不是复杂算法，而是少写文件：

```txt
1. 只有 package 有 bin 字段才处理
2. Windows 生成 .cmd/.ps1
3. Unix 生成 symlink 并确保 target chmod +x
4. shim 内容相同则不重写
5. 不要每次重建 .bin
```

你之前 macOS `Permission denied` 就是这个阶段的问题：`.bin/rollup` 找到了，但真实 target 没有可执行位。

Orix 应该做：

```rust
if package_has_bin {
    ensure_executable(package_bin_dest);
    create_bin_link_or_shim_if_changed();
}
```

Windows：

```txt
rollup.cmd
rollup.ps1
```

Unix/macOS：

```txt
node_modules/.bin/rollup -> ../.orix/rollup@x/bin/rollup
chmod +x target
```

---

# 10. Scripts 阶段：并发受控 + side effects cache

pnpm 有 `childConcurrency`，默认 5，用来限制同时构建 `node_modules` 的子进程数量。它还有 `sideEffectsCache`，默认开启：如果 pre/post install 脚本修改了包内容，pnpm 会把修改后的包保存到 global store，后续安装同一个包时可以复用预构建结果。([pnpm][1])

这个对有 native build 的包非常重要，例如：

```txt
esbuild
sharp
better-sqlite3
node-sass
```

Orix 阶段设计：

```txt
MVP:
  scripts 并发限制
  ignore-scripts
  allow/deny scripts

P1:
  side effects cache
  script output cache
  failed script diagnostics

P2:
  build approval / trust policy
```

核心是：

```txt
不要无限并发跑 scripts
不要重复构建已经构建过的包
```

---

# 11. Store 清理：不在 install 时做重 GC

pnpm 的 store 不会在每次 install 后立即清掉旧包；`pnpm store prune` 才会移除未引用包。文档也提醒不要太频繁 prune，因为切分支或安装旧依赖时可能又要重新下载。([pnpm][4])

这点对 Orix 也很重要。

不要在 install 结束时：

```txt
扫描整个 ~/.orix/store
删除未引用包
```

应该做：

```txt
orix store prune
orix store status
orix store path
```

install 阶段最多只写引用信息，不做全局 GC。

---

# 12. UI / Reporter 阶段：节流，不让日志破坏刷新

pnpm 的 UI 快感不是核心安装性能，但会影响体感。

你的 Orix 目前遇到过：

```txt
终端重复输出
extract debug 污染 UI
spinner 刷屏
```

正确策略：

```txt
1. pipeline 不 println/eprintln
2. debug 走 tracing 文件
3. progress 事件可丢，final/failed 事件不可丢
4. frame 刷新 80~120ms 节流
5. TTY 动态 UI，CI plain log
```

这个不一定是 pnpm 的核心算法，但它是现代包管理器必须做的体验优化。

---

# 13. pnpm 每阶段性能优化总结表

| 阶段          | pnpm 优化点                                             | Orix 应该怎么学                                  |
| ------------- | ------------------------------------------------------- | ------------------------------------------------ |
| lockfile      | lockfile 满足时 headless install，跳过 resolve          | lockfile validate 成功直接 resolve_from_lockfile |
| resolve       | 元数据缓存、并发请求、workspace 索引、peer 结果复用     | metadata cache + request dedupe                  |
| fetch         | networkConcurrency、retry、timeout、fetch from lockfile | download/extract 分离并发，支持 `oi fetch`       |
| cache         | tarball 缓存、integrity 校验                            | 原子写 cache，extract 失败 invalidate            |
| store         | content-addressable store                               | hash 文件入库，已存在跳过                        |
| import        | packageImportMethod auto：clone/hardlink/copy fallback  | 实现 Importer，记住跨盘 fallback                 |
| virtual store | `.pnpm` 固定深度布局                                    | `.orix` 固定深度，超长 key hash                  |
| link          | hardlink 文件 + symlink 依赖图                          | 不全量 unlink，增量 link                         |
| bin           | 平台 shim，Unix chmod                                   | Windows cmd/ps1，Unix chmod +x                   |
| scripts       | childConcurrency、sideEffectsCache                      | 控制并发，后续做 build cache                     |
| store gc      | 不每次 prune                                            | 单独 `oi store prune`                            |
| UI            | reporter 单独渲染                                       | crossterm 节流 + debug 文件                      |

---

# 14. 对你当前 Orix 最该抄的 5 个点

优先级最高的是这几个：

```txt
1. lockfile fast path
   lockfile 满足时跳过 resolve。

2. package import method auto
   macOS clonefile/reflink -> hardlink -> copy；
   Windows hardlink -> copy；
   并记住本次 fallback。

3. 不要全量 unlink node_modules
   graph hash 不变时直接跳过 link；
   graph 变了只处理 diff。

4. import 不再 WalkDir
   用 store integrity 文件列表导入。

5. sideEffects/bin/layout marker
   package.json 作为完成标记；
   .bin shim 内容一致不重写；
   layout marker 有效直接跳过。
```

---

# 15. 最终一句话

pnpm 快的核心不是“下载快”，而是：

```txt
lockfile 能跳过 resolve；
store 能复用内容；
import 能 clone/hardlink/copy 自动选择；
virtual store 避免重复文件；
link 只搭 symlink 图；
scripts 有并发和 side effects cache；
install 不做重 GC；
重复安装尽量什么都不做。
```

你现在 Orix 的性能优化主线应该从：

```txt
怎么更快创建 node_modules
```

转成：

```txt
怎么尽量不重建 node_modules
```

这才是和 pnpm 接近的方向。

[1]: https://pnpm.io/settings "Settings (pnpm-workspace.yaml) | pnpm"
[2]: https://pnpm.io/cli/fetch "pnpm fetch | pnpm"
[3]: https://pnpm.io/symlinked-node-modules-structure "Symlinked `node_modules` structure | pnpm"
[4]: https://pnpm.io/cli/store "pnpm store | pnpm"

---

# 16. Orix 并行安装管道设计

pnpm 的安装不是简单的串行：

```txt
resolve 完成 -> fetch 完成 -> link 完成
```

更准确的模型是一个有背压的流水线：

```txt
manifest / lockfile / workspace scan
  -> resolve package metadata
  -> emit resolved package
  -> fetch tarball if missing
  -> import package into store
  -> link package files into virtual store
  -> after graph edges known, link dependency symlinks and direct links
  -> write lockfile / modules metadata
```

也就是说，很多包不需要等整个 graph 完整解析后才开始下载。只要某个 package 的版本、tarball、integrity 已经确定，就可以进入 fetch/import 阶段。

## 阶段依赖关系

可以把 install 看成 DAG，而不是线性函数：

```txt
setup
  ├─ read config
  ├─ read root manifest
  ├─ discover workspace
  └─ read lockfile

resolve
  ├─ resolve metadata tasks
  ├─ produce ResolvedPackage events
  └─ produce graph edges

fetch/import
  ├─ consume ResolvedPackage events
  ├─ skip packages already complete in store
  ├─ download missing tarballs
  ├─ verify integrity
  └─ import files atomically into CAS store

link
  ├─ package file import can start per package after store import
  ├─ dependency symlink pass requires graph edges
  ├─ top-level direct links require final direct dependency set
  └─ workspace links require workspace index

finish
  ├─ validate layout
  ├─ run lifecycle scripts
  ├─ write lockfile
  └─ write install metadata
```

关键约束：

```txt
fetch/import 只依赖单个包的 resolution，不依赖完整 graph。
package files link 只依赖该包已在 store 中。
dependency symlink link 依赖 graph edges 和目标包已可见。
lockfile write 必须等 resolve 完整成功。
lifecycle scripts 必须等 link 完整成功。
```

## 设计目标

P0 目标不是极限快，而是把 Orix 从“全局阶段阻塞”改为“包级流水线”：

```txt
1. resolve 出一个包，就可以尝试 fetch/import 一个包。
2. store 已有完整包时，fetch/import 立即完成。
3. 所有 I/O 都有并发上限和背压。
4. 失败时不写 lockfile，不写成功 marker。
5. 半成品 tarball/store/import 通过 tmp + rename 保证可恢复。
```

## 新核心抽象

建议在 `crates/core/src/pipeline/install/` 下引入一个小型 scheduler，而不是把逻辑塞回 `mod.rs`：

```txt
install/
├── mod.rs
├── scheduler.rs        # 安装流水线调度
├── resolve_stream.rs   # resolver event -> ResolvedPackage stream
├── fetch_worker.rs     # download/extract/import workers
├── link_worker.rs      # package-level virtual-store import/link workers
└── finish.rs
```

核心事件：

```rust
enum InstallTaskEvent {
    PackageResolved(ResolvedPackage),
    PackageFetched(PackageId),
    PackageImported(PackageId),
    ResolveFinished(DependencyGraph),
    FetchFailed { id: PackageId, error: String },
}
```

核心状态：

```rust
struct InstallSchedulerState {
    graph: DependencyGraph,
    resolved: HashSet<PackageId>,
    fetched: HashSet<PackageId>,
    imported: HashSet<PackageId>,
    failed: Vec<InstallFailure>,
}
```

调度器持有 bounded channel：

```txt
resolved_tx/resolved_rx: 解析出的包
fetch_tx/fetch_rx: 待 fetch/import 的包
link_tx/link_rx: 待 package file link 的包
progress_tx: UI 事件
```

channel 必须 bounded，例如 1024，避免 resolve 太快时把内存打爆。

## 并行边界

### 可以并行

```txt
workspace discovery 和 lockfile read 可与 config 后的部分准备并行。
registry packument 请求可并发。
resolved package 的 tarball fetch 可与后续 resolve 并发。
tarball download 可与 extract/import 并发，但要分池。
不同 package 的 store import 可并发，但需要 store/package 级锁。
不同 package 的 bin metadata 读取可并发。
workspace direct link 可按 workspace package 并发，但 Windows 下要保守。
```

### 不能提前

```txt
不能在 resolve 完整成功前写 lockfile。
不能在 link 完整成功前写 layout marker。
不能在依赖目标包未 import 完成前创建依赖 symlink。
不能在 graph 未稳定前 prune 旧 virtual-store 包。
不能在 link 完整成功前运行 install/postinstall/prepare。
```

## 推荐流水线

### 阶段 1：setup

保持现在的串行/小并行即可：

```txt
Config.load
Manifest.read
Workspace.discover
Lockfile.read
```

这里最重要的是 fast path：

```txt
如果 lockfile importers 满足 root + workspace manifests：
  graph = resolve_from_lockfile(lockfile)
  跳过 registry resolve
  进入 fetch missing + link validate
```

### 阶段 2：streaming resolve

当前 resolver 是 `resolve_batch_concurrent(...) -> DependencyGraph`。需要演进成：

```rust
resolve_batch_streaming(
    initial_tasks,
    resolved_tx,
    progress_tx,
) -> DependencyGraph
```

每个包版本确定后立即发送：

```txt
ResolvedPackage -> resolved_tx
```

resolver 仍然负责：

```txt
name/range memo
PackageId dedupe
workspace protocol
catalog expansion
peer policy
platform optional filtering
```

### 阶段 3：fetch/import worker pool

fetch worker 消费 `ResolvedPackage`：

```txt
if !fetchable package:
  mark imported
else if store package complete:
  mark imported
else:
  download tarball if cache missing
  verify integrity
  extract to temp
  import to CAS store atomically
  mark imported
```

并发应该拆成两个池：

```txt
network_concurrency: 16~64
extract_import_concurrency: 4~16
```

不要用一个 `config.concurrency` 同时控制网络和磁盘。Windows 下 extract/import 更容易成为瓶颈。

### 阶段 4：link

link 分两层：

```txt
package file link:
  包 import 完成后，可以把 package files 放入 root node_modules/.orix/<key>/node_modules/<name>

dependency/direct/workspace symlink:
  graph 完整后统一建立
```

这样可以提前做重 I/O，但保留 symlink pass 的确定性。

伪流程：

```txt
fetch/import worker emits PackageImported(id)
link worker ensures .orix/<key>/node_modules/<name> exists

after ResolveFinished and all required PackageImported:
  link package internal dependency symlinks
  link root direct deps
  link workspace direct deps
  create .bin
  validate layout
  write marker
```

### 阶段 5：finish

必须保持事务尾部：

```txt
run dependency lifecycles
run project install/postinstall/prepare
write lockfile
emit final report
```

是否“先写 lockfile 再 scripts”需要单独决策。当前 Orix 是 link 后写 lockfile，并运行 scripts；短期保持现状，避免扩大行为变化。

## 错误与取消

任何 worker 失败后：

```txt
1. 广播 cancel token。
2. resolver 停止 spawn 新任务。
3. fetch/import worker drain 或退出。
4. link worker 不写 marker。
5. finish 不写 lockfile。
6. 临时目录由各阶段 cleanup。
```

推荐用 `tokio_util::sync::CancellationToken`。

错误优先级：

```txt
workspace protocol missing > invalid manifest/catalog > registry error > fetch error > store/link error
```

这样 `workspace:*` 缺包不会再被误报成 registry 404。

## 数据一致性

需要几个完成标记：

```txt
tarball cache:
  <hash>.tmp -> <hash>.tgz rename

store package:
  packages/<key>/files/...
  packages/<key>/integrity.json.tmp -> integrity.json

virtual store package:
  .orix/<key>/node_modules/<name>/...
  .orix/<key>/metadata.json

layout:
  node_modules/.orix/metadata.json
```

只有 marker 完整存在时，fast path 才能跳过对应阶段。

## UI 进度模型

并行流水线后，UI 不能只显示一个阶段的计数。建议 reporter 展示四个计数：

```txt
Resolving dependencies  resolved/discovered
Fetching packages       fetched/needed
Importing packages      imported/needed
Linking dependencies    linked/total
```

阶段可以同时处于 active：

```txt
● Resolving dependencies 1200/3400
● Fetching packages 850/1190
● Importing packages 700/1190
○ Linking dependencies
○ Writing lockfile
```

这比“resolve 没结束所以 fetch 是 0”更接近真实工作。

## 渐进实施顺序

不要一次性重写整个 install。建议分 5 个 PR：

```txt
PR 1: resolver 输出 PackageResolved event，但仍等待完整 graph 后 fetch。
PR 2: fetch/import worker 可消费 resolved event，先只做预取，不影响现有结果。
PR 3: resolve 与 fetch/import 真正并行，保留 link 串行。
PR 4: package file link 提前到 import 完成后，dependency symlink pass 仍在 graph 完成后。
PR 5: workspace direct link 与 bin shim 并行化，补 layout marker 粒度。
```

每一步都要有可回退开关：

```txt
ORIX_PIPELINE_MODE=serial|streaming
```

默认先 serial，CI/bonree 用 streaming 验证稳定后再翻默认值。

## P0 验收标准

在 bonree 这种 workspace 上，P0 不要求完全等同 pnpm，但应该达到：

```txt
1. 有 lockfile 且 importers 匹配时，不访问 registry resolve。
2. 无 lockfile 冷安装时，resolve 期间 fetch 已经开始。
3. workspace:* 缺包本地失败，不请求 registry。
4. 中途失败不写 lockfile，不写 layout marker。
5. 再次安装只 fetch/link 缺失项。
6. UI 能同时显示 resolving/fetching/importing/linking。
```

## 风险点

```txt
1. resolver 和 fetch 并行后，错误出现顺序可能变化。
2. 取消传播不完整会留下后台任务。
3. Windows 文件锁会放大并发 import/link 问题。
4. peer dependency 解析过早 fetch 可能拉入后续被 dedupe 掉的包。
5. workspace package 缺失必须优先失败，不能被 registry fallback 掩盖。
```

所以第一版 streaming 应该“多做 fetch 允许，少做 link 不允许”：

```txt
提前 fetch/import 是安全的，因为 store 可复用。
提前创建最终 symlink 要谨慎，因为它影响 Node 运行时可见布局。
```
