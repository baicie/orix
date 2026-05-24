# Bonree 安装性能优化方案

## 背景

Bonree 仓库是当前最接近真实生态压力的性能样本：

```txt
Scope: all 49 workspace projects
Packages: +4566
Progress: resolved 4894, reused 4422, downloaded 63, added 4566, done
Done in 2m 53.8s using pnpm v11.1.2
```

同一仓库下，`orix install` 目前可能耗时约 1500s。这个差距不应只理解成“某个函数慢”，而是安装管道缺少 pnpm 式的多层复用和跨阶段并行：

- lockfile 可用时仍可能走完整 resolve / link。
- resolve、fetch、store import、link 基本按阶段串行推进。
- workspace 场景会把 49 个 importer 的依赖一起放大，容易产生重复解析和重复链接。
- registry metadata、tarball、store、node_modules layout 的命中情况缺少统一度量。
- Windows 文件系统下，大量目录删除、硬链接、junction、`.bin` shim 创建会成为实际瓶颈。

本文档给出从“可观测”到“接近 pnpm 级别”的完整优化路线。

## 目标

### 性能目标

| 场景 | 当前观测 | 第一阶段目标 | 最终目标 |
| --- | ---: | ---: | ---: |
| Bonree 冷安装，无完整 store | 约 1500s | 5 分钟内 | 接近 pnpm 3 分钟 |
| Bonree 暖安装，lockfile + store 可用 | 待量化 | 60s 内 | 20-30s |
| Bonree lockfile headless install | 待量化 | 不访问 registry | 只补缺失 store / layout |

### 工程目标

- 保持 crate 边界：`core` 编排，`resolver` 解析，`registry` 元数据，`fetcher` 下载解压，`store` CAS，`linker` node_modules。
- 每个优化都能独立回滚，避免一次性重写安装管道。
- 优先修复重复工作，再扩大并发；否则会把错误工作并发放大。
- 所有阶段输出结构化耗时和计数，优化前后可比较。

## 非目标

- 不在本方案中实现完整 pnpm peerDependencies 算法。
- 不引入后台 daemon。
- 不改变 MVP 的严格 node_modules 结构目标。
- 不要求一次 PR 完成全部优化。

## 现状诊断

### 1. 缺少 Bonree 级别的阶段基线

用户现在只能看到最终耗时，无法判断 1500s 分布在：

- workspace discover
- resolve metadata
- tarball fetch
- extract
- store import
- virtual store link
- direct dependency link
- `.bin` shim
- lifecycle scripts
- lockfile write

没有阶段级数据时，容易把“Resolve 进度慢”误判成唯一瓶颈。

### 2. Lockfile fast path 需要覆盖 workspace importer

pnpm 在 lockfile 满足 package.json 时会优先走 headless install。对 Bonree 这种 49 workspace 项目，重复安装时最大的收益是跳过 registry resolve。

Orix 应把 lockfile 当成可重现 graph 的来源：

```txt
all package.json importer specs unchanged
lockfile format supported
registry / config key unchanged
store package marker valid
node_modules layout marker valid
```

满足时直接进入：

```txt
read graph from lockfile
  -> fetch/import missing packages only
  -> repair invalid links only
  -> run required scripts only
```

### 3. Resolve 应先去重再并发

Bonree 的 root + workspace direct dependencies 可达到数百个初始任务。如果只按 `(package, raw constraint)` 去重，会出现：

- 同一 package name 的多个 constraint 同时 miss cache。
- 同一 packument 被重复请求或重复 JSON parse。
- 同一版本最终解析出多个等价节点。
- workspace 协议缺失时错误地回退到 registry。

需要分清三层 key：

| 层级 | Key | 作用 |
| --- | --- | --- |
| Packument | registry + package name | HTTP 请求 single-flight |
| Version index | package name + packument revision | JSON parse 和版本排序复用 |
| Resolution | package name + normalized constraint + peer context | 版本选择结果复用 |

### 4. Fetch / Import / Link 串行化浪费等待时间

pnpm 不会等整个世界 resolve 完才开始后续阶段。Orix 也应把安装拆成流水线：

```txt
resolved package
  -> fetch tarball if missing
  -> extract if missing
  -> import store if missing
  -> prepare package files in virtual store

full graph resolved
  -> create dependency symlinks
  -> create direct dependency links
  -> create .bin
  -> write lockfile / modules metadata
```

能提前做的事情提前做，必须等完整 graph 的事情留到最后。

### 5. Linker 在 Windows 上需要避免全量重建

Bonree 有 4566 个 package。Windows 上删除 stale link、创建 junction、硬链接文件、创建 shim 都很贵。优化重点不是把 unlink 做得更快，而是减少 unlink / relink：

- graph 未变时不重建 virtual store。
- package 文件已导入时不重复从 store 链接。
- workspace importer 只创建 direct link，不复制一份 package files。
- `.bin` shim 内容未变时不重写。
- stale 清理基于 manifest/layout marker，而不是扫描和删除整棵树。

## 目标架构

### 管道总览

```txt
read config / manifests / workspace / lockfile
  |
  +-- lockfile valid? ------------------ yes --> graph from lockfile
  |                                             |
  no                                            v
  |                                      missing package queue
  v                                             |
streaming resolver                              v
  |                                      fetch / extract workers
  v                                             |
resolved package queue ------------------------+
  |
  v
store import workers
  |
  v
package file link workers
  |
  v
graph finalization
  |
  v
dependency symlink pass / direct links / bins / lockfile / scripts
```

### 可并行与不可并行边界

| 工作 | 是否可提前 | 说明 |
| --- | --- | --- |
| packument fetch | 是 | package name single-flight |
| semver select | 是 | 依赖 package metadata |
| tarball fetch | 是 | 只依赖 resolved package metadata |
| extract | 是 | 只依赖 tarball |
| store import | 是 | 只依赖 extracted package |
| package files link 到 virtual store | 是 | 只依赖 store package |
| dependency symlink | 否 | 需要完整 graph 和 peer context |
| direct dependency link | 半可提前 | importer direct set 已知后可做，但建议 final pass 校验 |
| `.bin` | 半可提前 | package files 完成即可创建，final pass 校验 |
| lockfile write | 否 | 需要最终 graph |
| lifecycle scripts | 否 | 需要可用 node_modules |

### 并发配置

不要用一个 `concurrency` 控制所有阶段。网络、解压、store、link 的瓶颈不同，建议拆分：

```toml
[install]
metadata-concurrency = 32
download-concurrency = 32
extract-concurrency = 8
import-concurrency = 8
link-concurrency = 8
script-concurrency = 4
```

默认值可以由 CPU 核数和平台决定。Windows 下 `link-concurrency` 不宜过高，否则硬盘随机 I/O 和杀毒扫描会互相拖慢。

## 分阶段实施方案

## P0：可观测性与 Bonree 基准

### 方案

新增结构化性能报告，至少包含：

| 字段 | 含义 |
| --- | --- |
| `workspace_count` | workspace importer 数量 |
| `direct_dependency_count` | root + workspace direct dependency 总数 |
| `resolved_package_count` | graph package 数 |
| `metadata_requests` | registry packument 请求数 |
| `metadata_cache_hits` | metadata 缓存命中数 |
| `tarball_downloads` | 实际下载 tarball 数 |
| `tarball_cache_hits` | tarball 缓存命中数 |
| `store_imports` | 实际导入 store 包数 |
| `store_package_hits` | store package 命中数 |
| `linked_packages` | 实际链接 package 数 |
| `reused_links` | layout 复用数 |
| `resolve_ms` | resolve 耗时 |
| `fetch_ms` | fetch 耗时 |
| `extract_ms` | extract 耗时 |
| `import_ms` | store import 耗时 |
| `link_ms` | linker 耗时 |
| `scripts_ms` | lifecycle scripts 耗时 |
| `total_ms` | 总耗时 |

输出形式：

```txt
ORIX_PERF=1 oi i --debug
```

或：

```txt
oi i --report-json .orix/install-report.json
```

### 验收

- Bonree 冷安装、暖安装、`oi prune --keep-lockfile` 后安装各有报告。
- 能明确 1500s 的最大阶段。
- CI fixture 覆盖 report 字段稳定存在。

## P1：Lockfile headless install

### 方案

实现 workspace 级 lockfile 校验：

```txt
for each importer:
  package.json dependency specs == lockfile importer specs
  workspace package name/path matches lockfile
  supported lockfile version
```

命中后：

```txt
DependencyGraph::from_lockfile(lockfile)
  -> skip registry resolver
  -> validate workspace links locally
  -> fetch/import missing packages
```

对于 `workspace:*`、`workspace:^`、`workspace:~`：

- 若 workspace 中存在对应 package，解析成本地 workspace link。
- 若不存在，直接报 `workspace dependency ... was not found in the workspace`。
- 禁止回退到 registry。

### 验收

- Bonree lockfile 未变时，metadata request 数为 0。
- `oi prune --keep-lockfile && oi i` 不再进入完整 registry resolve。
- 缺失 `@bonree/common` 这类 workspace 依赖时，本地快速失败，不请求 npm registry。

## P2：Registry metadata 持久缓存与 single-flight

### 方案

在 `registry` crate 增加磁盘 packument 缓存：

```txt
~/.orix/cache/metadata/<registry-hash>/<escaped-package-name>.json
~/.orix/cache/metadata/<registry-hash>/<escaped-package-name>.meta.json
```

缓存元信息：

```json
{
  "etag": "...",
  "last_modified": "...",
  "fetched_at": "...",
  "registry": "https://registry.npmjs.org/"
}
```

同一次进程内增加 single-flight：

```txt
package name cache miss
  -> check in_flight_packuments
  -> join existing future if exists
  -> otherwise start HTTP request
```

解析后生成 compact version index：

```txt
package name
  -> sorted versions
  -> dist tags
  -> dependency metadata by version
```

semver 匹配复用该 index，避免每个 constraint 重复遍历和 parse 大 JSON。

### 验收

- 同一 package name 并发解析时只产生一次 HTTP 请求。
- 第二次完整 resolve 基本不下载 packument body。
- `metadata_requests`、`metadata_cache_hits` 可在 report 中看到。

## P3：Streaming resolve -> fetch -> import

### 方案

将 resolver 从“返回完整 graph”扩展为可选 streaming API：

```rust
pub enum ResolveEvent {
    PackageResolved(ResolvedPackage),
    PackageSkipped(PackageId),
    Progress(ResolveProgress),
    Finished(DependencyGraph),
}
```

`core` 侧建立 bounded channel：

```txt
resolver task
  -> resolved_package_tx
  -> fetch workers
  -> import workers
  -> package_link workers
```

要求：

- channel 有界，避免 4000+ package 全部堆内存。
- 任一 worker 出错后通过 cancellation token 停止全管道。
- `Finished(DependencyGraph)` 到达前，不做最终 dependency symlink。
- progress UI 展示多个阶段同时进行。

### 验收

- Bonree 冷安装中，resolve 未结束时 fetch/import 已开始。
- report 能显示阶段重叠时间。
- 失败时不会留下半写 lockfile。

## P4：Fetch / Extract / Store import 分层复用

### 方案

把“包是否已准备好”拆成三个 marker：

```txt
tarball cache marker:
  url + integrity -> tgz exists and verified

extract marker:
  tarball digest + package id -> extracted dir complete

store package marker:
  package id + integrity + file index -> store package complete
```

重复安装时：

```txt
store package hit -> skip tarball/extract/import
tarball hit but store miss -> extract/import
tarball miss -> download/extract/import
```

Windows 下记录本次安装的链接能力：

```txt
hardlink ok for volume pair -> continue hardlink
hardlink failed with EXDEV/permission -> fallback copy for same pair
```

避免每个文件都重复试错。

### 验收

- Bonree `reused 4422` 类场景中，store hit 数接近已有包数。
- 暖安装不重复解压已导入包。
- Windows fallback 不产生大量重复 warning。

## P5：Linker 增量化

### 方案

新增 layout marker：

```txt
node_modules/.orix-state.json
```

内容包含：

```json
{
  "graph_hash": "...",
  "importers": {
    ".": "...",
    "example": "...",
    "playground/backend": "..."
  },
  "virtual_store_dir": "node_modules/.orix",
  "linker_version": 1
}
```

Linker 执行策略：

```txt
graph hash unchanged
  -> validate marker
  -> skip full link

package virtual store entry exists and package hash unchanged
  -> skip package file relink

importer direct dependency set changed
  -> update only that importer's direct links

bin target unchanged
  -> skip shim rewrite
```

workspace 项目统一复用 root virtual store：

```txt
root/node_modules/.orix/<pkg>
workspace/node_modules/<dep> -> ../../node_modules/.orix/<pkg>/node_modules/<dep>
```

不要为每个 workspace 重复导入或重复构建 package 文件树。

### 验收

- graph 未变时，Bonree link 阶段进入秒级校验。
- 修改单个 workspace package.json，只更新对应 importer direct links。
- Windows stale direct dependency link 删除失败时，能区分权限占用和路径类型问题，并给出具体路径。

## P6：Lifecycle scripts 与 side effects

### 方案

短期只做安全并发：

- 按拓扑顺序运行需要 build 的 package。
- `script-concurrency` 限制并发。
- stdout/stderr 归属 package，避免日志交错不可读。
- 失败后保留完整上下文。

中期支持 side effects cache：

```txt
package id + platform + node version + script hash
  -> side effects cache key
```

命中后复用 build 产物，避免重复执行 native build。

### 验收

- Bonree 默认脚本耗时可在 report 中单独看到。
- 不需要 build 的包不进入 script 队列。
- script 失败不会污染成功的 store marker。

## P7：进度 UI 与 debug 日志降噪

### 方案

进度模型从单阶段改成多阶段并行：

```txt
✓ Resolved dependencies 4894 packages (1678 scanned)
● Fetched packages 1019/4566
● Imported packages 870/4566
○ Linking dependencies
○ Running scripts
```

debug log：

- 默认按阶段聚合，不对每个文件输出。
- package 级事件保留 trace 模式。
- report 中记录慢包 top N：

```txt
slow_metadata
slow_download
slow_import
slow_link
slow_script
```

### 验收

- 大仓库安装时 UI 不因频繁刷新拖慢。
- debug 信息足以定位慢包，但普通 debug log 不膨胀到不可读。

## 推荐 PR 顺序

1. **Perf report**：增加结构化阶段计时和 Bonree 手工基准说明。
2. **Workspace lockfile fast path**：覆盖多 importer，缺失 workspace 协议本地失败。
3. **Registry single-flight**：进程内 packument 请求去重。
4. **Metadata disk cache**：ETag / Last-Modified / TTL 缓存。
5. **Fetch/import marker**：store package hit 时跳过 tarball/extract/import。
6. **Streaming pipeline prototype**：先 behind flag：`ORIX_PIPELINE=streaming`。
7. **Incremental linker marker**：graph unchanged 时跳过全量 link。
8. **Workspace root virtual store reuse**：避免 workspace 重复 package files。
9. **Lifecycle script concurrency**：独立计时和并发控制。
10. **默认启用 streaming pipeline**：在 Bonree 和 CI fixture 稳定后切换默认。

## 测试计划

### 单元测试

- resolver：
  - 同一 package name 多 constraint 只 fetch 一次 packument。
  - `workspace:*` 缺失时不访问 registry。
  - version index 被多个 constraint 复用。
- registry：
  - ETag 命中返回 cached body。
  - 并发 cache miss single-flight。
  - 缓存损坏时自动回退网络。
- fetcher/store：
  - tarball hit、extract hit、store hit 三类 marker。
  - integrity 不匹配时删除坏缓存。
- linker：
  - graph hash 未变跳过 full link。
  - importer direct set 改变只更新对应 importer。
  - Windows junction stale cleanup。

### 集成测试

- 3 个 workspace importer 共享同一依赖，只生成一份 virtual store package。
- lockfile 满足所有 importer 时，registry request count 为 0。
- 修改一个 workspace package.json，只触发局部 resolve / link。
- `oi prune --keep-lockfile` 后安装能从 lockfile 恢复 graph。

### 性能测试

Bonree 手工基准至少保留三组：

```txt
cold:
  empty node_modules, empty relevant store/cache

warm-store:
  empty node_modules, store/cache retained

headless:
  node_modules pruned, lockfile/store/cache retained
```

每组记录：

```txt
pnpm i timing
oi i timing
oi i --report-json
git rev
node version
os / disk type
registry
```

## 风险与回滚

### Streaming pipeline 风险

风险：阶段重叠后，错误处理和清理更复杂。

回滚：

```txt
ORIX_PIPELINE=serial
```

保留旧串行路径直到 Bonree、Windows、CI fixture 稳定。

### Metadata cache 风险

风险：缓存过期策略错误导致拿到旧 metadata。

回滚：

```txt
ORIX_DISABLE_METADATA_CACHE=1
```

默认遵守 registry 的 ETag / Last-Modified；没有缓存头时使用短 TTL。

### Link marker 风险

风险：错误跳过 link 导致 node_modules 缺文件。

回滚：

```txt
oi i --force-link
```

marker 校验必须保守：不确定就重建，不能假装命中。

## 成功标准

第一阶段成功：

- Bonree 暖安装进入 60s 内。
- `oi prune --keep-lockfile && oi i` 不进行 registry resolve。
- report 能解释 80% 以上耗时来源。

最终成功：

- Bonree 冷安装稳定接近 pnpm 的 3 分钟级别。
- Bonree 暖安装稳定进入 20-30s。
- 大 workspace 下不再出现 resolve / fetch / link 单阶段长时间静默。
- 性能优化不牺牲可复现性：lockfile、integrity、workspace 协议错误仍严格校验。

