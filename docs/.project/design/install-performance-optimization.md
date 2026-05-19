# Install 性能优化方案

## 背景

一次 `orix install` 输出示例：

```txt
Packages: +50 -0
Registry: https://registry.npmmirror.com/

✓ Resolved dependencies 50/50
✓ Fetched packages 50/50
✓ Linked dependencies
✓ Lockfile written

Done in 47s
```

当前 UI 只展示最终状态，没有展示各阶段耗时。因此 `Done in 47s` 不能直接归因给 Resolve。需要先补齐阶段级计时，再根据数据决定优化顺序。

本文档从当前代码实现出发，整理 `install` 全链路的可优化点、优先级和验收方式。

## 目标

- 明确 47s 分布在 Resolve、Fetch、Store Import、Link、Lockfile、Lifecycle 哪些阶段。
- 让重复安装尽量命中 lockfile / store / node_modules 快路径。
- 让首次安装的网络 I/O 并发化、去重化。
- 让文件系统 I/O 避免不必要的全量重建。
- 保持现有 crate 边界和无环依赖方向。

## 非目标

- 不在本阶段实现完整 peerDependencies 算法。
- 不改变 lockfile 格式。
- 不引入后台 daemon。
- 不把 registry packument resolve 和 tarball fetch 合并为同一阶段。

## 当前链路

```txt
Config / Manifest / Workspace / Lockfile
  -> Resolver              # packument + semver + DependencyGraph
  -> Fetcher               # tarball cache / download / extract
  -> Store.import_package  # CAS 文件级导入
  -> Linker.unlink
  -> Linker.link_graph     # node_modules/.orix + hardlink/symlink
  -> Lockfile.write
  -> Lifecycle scripts
```

当前已有一个 lockfile fast path：

- lockfile 与 manifest 匹配，且不是 `--force` / `--frozen-lockfile` 时，从 lockfile 构建 graph。
- fast path 会跳过 registry resolver。
- fast path 会只 fetch store 中缺失的包。
- fast path 目前仍会继续重建 `node_modules`。

## 第一优先级：补齐阶段耗时

### 问题

当前用户只能看到 `Done in 47s`，无法判断瓶颈阶段。Resolve UI 结束很快或很慢都可能被后续 Fetch / Link 掩盖。

### 方案

在 `crates/core/src/pipeline.rs` 中对阶段计时：

| 阶段 | 计时范围 |
| --- | --- |
| `resolve_ms` | PhaseStarted(Resolve) 到 Resolved |
| `fetch_ms` | PhaseStarted(Fetch) 到 PhaseFinished(Fetch) |
| `link_ms` | PhaseStarted(Link) 到 PhaseFinished(Link) |
| `lockfile_ms` | PhaseStarted(Lockfile) 到 Lockfile event |
| `scripts_ms` | lifecycle scripts 执行总耗时 |
| `total_ms` | install start 到 Finished |

输出可以先只在 debug / verbose 模式展示：

```txt
Timing: resolve 0.8s, fetch 31.2s, link 14.6s, lockfile 0.1s
```

也可以在 `InstallReport` 中增加结构化字段，供后续 JSON 输出和 benchmark 使用。

### 验收

- 普通 `orix install` 不增加噪音，或只在最终摘要中一行展示。
- debug 模式能看到完整阶段耗时。
- 测试覆盖 fast path 与普通 path 都能生成 timing。

## Resolve 优化

### 当前状态

`crates/resolver/src/resolver.rs` 已经有并发调度器：

- `JoinSet` 管理正在解析的任务。
- `Semaphore` 限制并发度。
- `core` 创建 resolver 时传入 `config.concurrency`。

因此 Resolve 已经不是原始串行 DFS。剩余优化重点是去重、缓存和 CPU 小热点。

### 1. Registry packument single-flight

#### 问题

`RegistryClient::fetch_packument` 当前流程是：

```txt
check memory cache
  -> cache miss
  -> acquire semaphore
  -> HTTP request
  -> insert cache
```

如果同一个 package name 因不同 constraint 同时进入解析任务，多个任务可能同时 cache miss，然后发起重复 packument 请求。

当前 resolver 的 `in_flight` key 是 `(PackageName, raw_constraint)`，不能保证同一 package name 的 packument 请求 single-flight。

#### 方案

在 `crates/registry` 增加 package name 级 single-flight：

```txt
in_flight_packuments: package name -> shared pending request
```

行为：

- 第一个请求负责真正 HTTP fetch。
- 后续同名请求等待同一个结果。
- 成功后写入内存 cache。
- 失败时所有等待者收到同一错误，in-flight entry 移除。

#### 收益

- 减少重复 registry metadata 请求。
- 对多个 range 指向同一包、workspace 多 manifest 的场景收益明显。
- 不改变 resolver 公共接口。

### 2. Resolver memo 语义修正

#### 问题

当前 `state.memo` 会被检查，但任务完成后没有明确写入：

```txt
(PackageName, raw_constraint) -> PackageId
```

实际去重更多依赖 `in_flight` 永久保留 key。这让 `memo` 和 `in_flight` 的职责不清晰。

#### 方案

任务完成后写入：

```txt
state.memo.insert((name, constraint.raw), pkg_id)
state.in_flight.remove(key)
```

并保留 graph 层面对 `PackageId` 的去重。

#### 验收

- 同一 `(name, raw_constraint)` 重复出现，只产生一次 resolution task。
- 同一 resolver 解析多个 manifest 时，后续 manifest 能复用 memo。
- `in_flight` 只表示正在执行，不表示已完成。

### 3. 版本选择避免排序

#### 问题

range 版本选择当前会收集候选版本、排序、再取最大值。

#### 方案

遍历 packument versions 时维护当前最大匹配版本：

```txt
best = None
for version in versions:
  if range.matches(version) && version > best:
    best = version
```

#### 收益

- 避免临时 Vec 分配和排序。
- 对版本很多的热门包有小收益。
- 实现风险低。

### 4. 磁盘 packument cache

#### 问题

packument cache 当前只在进程内有效。重复 install 只要没有命中 lockfile fast path，就仍需要 registry metadata 请求。

#### 方案

在 `crates/registry` 增加磁盘 cache：

```txt
<cache_dir>/packuments/<registry-host>/<package-key>.json
```

语义：

- 默认 TTL 可先沿用 5 分钟。
- `--force` 绕过并刷新。
- `--offline` 只允许读取磁盘 cache，未命中则报错。
- auth token 场景必须避免跨 token 复用私有包 metadata。

#### 优先级

中到高。对重复安装和 CI 网络抖动有效，但需要小心 cache key 与 auth 安全。

## Fetch / Store 优化

### 1. Store import 全局写锁过粗

#### 问题

`Fetcher::fetch_all` 会并发处理 tarball，但 `Store::import_package` 持有全局写锁覆盖整个导入过程：

```txt
walk extracted dir
read file
hash
copy CAS file
hardlink package file
write integrity.json
```

这会让下载完成后的 store import 基本串行。50 个包首次安装时，这可能比 Resolve 更接近 47s 的主因。

#### 方案 A：缩短全局锁范围

把 import 拆成两段：

```txt
outside lock:
  walk extracted dir
  read file
  compute hash
  prepare file index

inside lock:
  create missing CAS files
  create package hardlinks
  write integrity.json
```

#### 方案 B：按 package / content hash 加锁

长期可以用更细粒度锁：

- package key lock：防止同一包并发写 package entry。
- content hash lock：防止同一 CAS 文件并发创建。
- store read 不需要等待其他 package import 完成。

#### 验收

- 多个不同 package 的 hash 计算可以并发。
- 同一 package 并发 import 仍安全。
- `cargo test -p orix-store` 覆盖并发导入。
- 50 包 mock tarball 导入耗时相对全局锁版本下降。

### 2. Fetch progress 区分 download / extract / import

#### 问题

当前 UI 只显示 `Fetched packages 50/50`。用户无法知道慢在下载、解压还是 store import。

#### 方案

扩展 fetch event：

```txt
DownloadStarted / DownloadFinished
ExtractStarted / ExtractFinished
ImportStarted / ImportFinished
```

第一版不一定全部展示给普通用户，但 debug timing 应记录。

## Linker 优化

### 1. 避免每次全删 node_modules

#### 问题

安装流程在 link 前调用：

```txt
linker.unlink()
linker.link_graph(...)
```

`unlink` 直接删除整个 `node_modules`。即使 lockfile unchanged、store 完整，仍然会全量重建链接树。

#### 方案

增加 linker fast path：

```txt
if lockfile valid
  && node_modules marker matches lockfile graph hash
  && direct dependency symlinks valid:
    skip unlink + link_graph
```

可以在 `node_modules/.orix/metadata.json` 写入：

```json
{
  "lockfile_hash": "...",
  "graph_hash": "...",
  "orix_version": "..."
}
```

#### 验收

- 第二次 `orix install` 在 lockfile unchanged 且 layout valid 时跳过 link。
- 修改 package.json 后重新 link。
- 删除某个 direct symlink 后能检测并修复。

### 2. 增量 link / prune stale

#### 问题

全量重建对大项目 IO 很重。

#### 方案

用 graph diff 驱动：

- 新增 package：只创建对应 `.orix/<pkg>` 和依赖 symlink。
- 移除 package：删除 stale `.orix/<pkg>`。
- unchanged package：跳过文件硬链接。
- direct deps 变化：只更新根 `node_modules/<dep>` symlink。

#### 优先级

高，但建议在 linker fast path 后做。fast path 能快速解决重复 install，增量 link 解决 add/remove 和部分变更。

### 3. Linker 并发化

#### 问题

`link_graph` 当前逐包、逐文件串行 hardlink。

#### 方案

在 package 粒度并发 hardlink，限制并发度，避免压垮文件系统：

```txt
package link tasks buffered by concurrency
```

#### 注意

并发 link 的收益依赖文件系统。先做 timing 和 fast path，再决定是否值得。

## Lockfile / Fast Path 优化

### 当前 fast path

lockfile valid 时已经跳过 resolver，并只 fetch missing packages。

### 可扩展点

1. **store 完整时跳过 fetch phase 的大部分工作**
   - 当前 `fetch_only_missing` 会逐包 `store.contains`。
   - 可以保留，但输出应显示 `Fetched packages 0/0` 或 `Store complete`，避免误解。

2. **layout valid 时跳过 link**
   - 与 linker marker 结合。

3. **frozen-lockfile 默认策略**
   - CI 中应建议 `orix install --frozen-lockfile`，完全避免 resolver。

## 推荐落地顺序

### Phase 1：可观测性

1. 增加 install phase timing。
2. 增加 debug tracing 字段：
   - resolve package / constraint / duration / cache_hit
   - fetch download / extract / import duration
   - link package / file_count / duration
3. 输出最终 timing 摘要。

### Phase 2：Resolve 小闭环

1. 修正 resolver memo / in_flight 职责。
2. 增加 registry packument single-flight。
3. 版本选择改为单次遍历取 max。
4. 增加 mock registry 延迟测试：
   - 50 unique packages
   - concurrency 10
   - 验证耗时接近 `ceil(50 / 10) * RTT`
   - 验证同名 packument 不重复请求。

### Phase 3：Store import 并发收益

1. 缩短 `Store::import_package` 全局写锁范围。
2. 增加 package/content 级并发安全测试。
3. 用 fixture tarballs 跑 50 包导入 benchmark。

### Phase 4：Linker fast path

1. 增加 node_modules marker。
2. layout valid 时跳过 unlink/link。
3. 删除 direct symlink 的回归测试。
4. lockfile unchanged 的重复 install 应明显变快。

### Phase 5：磁盘 packument cache 与增量 link

1. packument disk cache，处理 TTL / force / offline / auth。
2. linker prune stale + package 粒度增量更新。

## 验收指标

### 首次安装

以 50 个唯一 package、registry RTT 100ms、并发 10 为例：

- Resolve 不应接近 `50 * RTT`。
- 目标接近 `ceil(50 / 10) * RTT + semver CPU`。
- Fetch/Store/Link 需要单独计时确认。

### 重复安装

lockfile unchanged、store 完整、node_modules layout valid 时：

- Resolve：fast path，接近 0。
- Fetch：missing packages 为 0。
- Link：fast path，跳过全量重建。
- 总耗时应主要是读取 manifest/lockfile 和 layout marker。

### 修改少量依赖

`orix add/remove` 或 package.json 小变更：

- 只解析受影响依赖闭包。
- 只 fetch/store 新增包。
- 只 link 新增/删除/变化的 package。

## 风险

| 风险 | 缓解 |
| --- | --- |
| single-flight 错误导致请求挂死 | in-flight entry 必须在成功/失败后移除；测试失败广播 |
| 磁盘 packument cache 泄露私有包 metadata | cache key 包含 registry/auth scope；默认不缓存带 auth 的响应，或隔离 token scope |
| Store 细粒度锁引入竞态 | 保留同包并发 import 回归测试；先缩短锁范围再细化锁 |
| Linker fast path 漏修损坏 layout | marker 之外必须验证 direct symlink；debug 命令可强制 validate |
| Windows symlink/junction 差异 | CI 覆盖 Windows；fast path 不假设 Unix 行为 |

## 结论

Resolve 阶段已经有并发基础，下一步高收益点是 packument single-flight 和 memo 语义修正。但 `Done in 47s` 更可能由 Fetch/Store/Link 混合造成，尤其是：

- Store import 全局写锁导致导入串行。
- Linker 每次删除并全量重建 `node_modules`。

因此建议先补阶段耗时，再按数据推进 Resolve、Store、Linker 三条线。
