# Resolver 性能优化设计

## 背景

当前 `orix install` 在 Resolve 阶段容易表现为长时间卡顿，尤其是首次安装、lockfile 不存在、依赖树较深或 registry 网络延迟较高时。Resolve 阶段的职责是从 root manifest / workspace manifest 出发，获取 npm packument，选择满足 semver 的版本，并构建完整 `DependencyGraph`。

现有设计文档已经要求 resolver 支持“并行获取 packument”，但当前实现仍接近串行深度优先解析：

- `crates/resolver/src/resolver.rs` 的 `resolve_batch_impl` 使用 `while let Some(...) = pending.pop()` 逐个处理依赖。
- 每个依赖都会在循环内 `await registry.fetch_packument(&name)`，下一个依赖必须等待上一个网络请求完成。
- peer dependency 解析调用 `fetch_packument_sync`，其内部通过当前 Tokio runtime `block_on` 异步请求，容易把异步执行路径重新阻塞住。
- packument 缓存只有内存 TTL 缓存，冷启动、跨进程安装或重复运行时无法复用 registry 元数据。
- 版本选择每次 range 匹配都会遍历并解析 packument 中所有版本字符串，热门包版本很多时 CPU 消耗明显。

因此，Resolve 卡顿的主要瓶颈不是单个 semver 匹配，而是“依赖发现和网络获取串行化”，并叠加 peer 同步查询、缓存粒度不足和进度反馈不准确。

## 目标

### 用户可感知目标

- 首次安装时 Resolve 阶段持续输出进度，不出现长时间静默。
- 对常见项目，Resolve 阶段耗时接近 `ceil(唯一 packument 数 / 并发度) * 平均 RTT`，而不是 `唯一 packument 数 * 平均 RTT`。
- 重复安装优先走 lockfile fast path；必须重新解析时，也尽量复用本地 packument 缓存。

### 工程目标

- 保持现有 crate 边界：并发解析仍在 `crates/resolver`，HTTP 细节仍在 `crates/registry`。
- 不改变 `DependencyGraph` 对 fetcher / linker / lockfile 的公共消费方式。
- 避免循环依赖，避免把 core 的安装流程细节下沉到 resolver。
- 并发度可配置，默认复用 `Config.concurrency`，必要时未来可增加 `resolve_concurrency`。
- 所有性能优化必须有基准、回归测试或可复现实验入口。

## 非目标

- 不在本阶段实现完整 pnpm peerDependencies 算法。
- 不改变 lockfile 格式。
- 不引入全局数据库或后台 daemon。
- 不把 fetch tarball 和 resolve packument 合并为同一阶段。

## 现状诊断

### 1. Resolve 主循环串行等待网络

当前流程简化如下：

```rust
while let Some((name, constraint)) = pending.pop() {
    if memo.contains_key(&key) {
        continue;
    }

    let packument = registry.fetch_packument(&name).await?;
    let version = select_version_impl(&packument, &constraint)?;
    let metadata = packument.versions.get(&version.to_string())?;

    graph.insert(resolved);
    memo.insert(key, pkg_id);

    for dep in metadata.dependencies {
        pending.push(dep);
    }
}
```

这会导致依赖树的每个新包都等待一次网络往返。即使两个依赖互不相关，也无法并发解析。

### 2. memo key 不足以阻止所有重复请求

当前 memo key 是 `(PackageName, raw_constraint)`。它可以避免同一包同一约束重复解析，但存在两个性能问题：

- `react@^18.2.0` 和 `react@18.2.0` 会作为两个 resolution key，虽然最终版本相同。
- 如果两个任务并发开始解析同一个 package name，只有“完成后”的 memo 才生效，无法去重“进行中的 packument 请求”。

因此需要区分两个层级：

- packument cache：按 package name 去重 HTTP 请求。
- resolution memo：按 package name + constraint 去重版本选择结果。

### 3. peer dependency 使用同步查询

`resolve_peer_dep` 当前调用 `registry.fetch_packument_sync(peer_name)`，而 `fetch_packument_sync` 会在当前 runtime 上 `block_on(self.fetch_packument(name))`。

这个路径的问题：

- 在 async resolver 中重新 block_on，容易造成调度卡顿甚至潜在死锁风险。
- 每个未命中的 peer 都可能引入额外网络 hop。
- peer 解析结果目前没有写入 graph，只返回 `PackageId`，对完整图价值有限，性能成本却实际存在。

MVP 阶段更合理的行为是：peer 只做诊断或延迟校验，不在主 resolve 热路径里同步网络查询。

### 4. packument 缓存只在进程内有效

`PackumentCache` 是内存 TTL 缓存。它能避免一次 install 中重复获取同一个包，但不能解决：

- 冷启动重复安装。
- 多 workspace 包反复 install。
- CI 中网络抖动造成的 Resolve 慢。

registry 元数据适合增加磁盘缓存，但必须尊重 TTL、ETag 或 cache-control，并处理 auth token 场景。

### 5. 版本索引重复构建

`select_version_impl` 对 range 约束会：

1. 遍历 `packument.versions.keys()`。
2. 每次解析版本字符串为 `Version`。
3. 排序后取最大满足项。

对版本数量多的包，这部分会反复发生。虽然一般小于网络耗时，但并发化之后 CPU 选择版本会变成次级瓶颈。

## 推荐方案

采用三层优化，按收益和风险分阶段落地：

1. **并发解析调度器**：把串行 DFS 改为有界并发 work queue。
2. **请求去重与缓存强化**：加入 in-flight packument 去重、磁盘 packument cache 和版本索引。
3. **peer 与 lockfile 快路径优化**：peer 不阻塞主解析；可从 lockfile 增量复用已解析节点。

## 最快策略

如果目标是尽快缓解“Resolve 阶段卡顿”，不要先做磁盘缓存、ETag、lockfile 增量复用或完整 peer 算法。最快路径是只改热路径上的三个点：

1. **把 `resolve_batch_impl` 改成有界并发 work queue**
   - 直接复用现有 `Config.concurrency`，默认并发 10。
   - 保留当前 `DependencyGraph`、`ResolvedPackage`、lockfile 输出格式。
   - 只要求最终 graph 稳定，不要求解析完成顺序稳定。

2. **移除主路径里的 peer 同步网络查询**
   - `resolve_peer_dep` 不再调用 `fetch_packument_sync`。
   - peer 在 MVP 中只做“已满足/未满足”诊断，不主动拉新包。
   - 这是最小风险的大收益改动，因为当前设计已经声明 MVP 不实现完整 peer 算法。

3. **加入最小 in-flight 去重**
   - 在 resolver 内先维护 `in_flight_resolution: HashSet<(PackageName, String)>`，避免同一约束重复派发。
   - packument single-flight 可以先不做成 registry 公共能力；若同一 package 不同 constraint 并发重复请求明显，再下沉到 `RegistryClient`。

这条路线不追求“一次做到最完善”，只追求最短时间让 Resolve 从串行网络等待变成并发网络等待。

### 优化项评估

结合当前实现，常见优化项的判断如下：

| 优化 | 预期效果 | 难度 | 结论 |
| --- | --- | --- | --- |
| 并发 fetch packument | 首次 resolve 可接近 10x 提速，取决于网络 RTT、依赖数量和并发度 | 中 | **第一优先级**。当前瓶颈正是串行 `await fetch_packument` |
| 磁盘 packument 缓存 | 二次安装或重复 resolve 可减少甚至消除 registry 元数据请求 | 小到中 | **第二优先级**。对重复安装有效，但不解决首次安装的串行等待 |
| batch search API | 理论上减少请求数 | 中 | **不建议作为通用方案**。npm search API 不能替代 resolver 所需的完整 packument |
| frozen-lockfile 跳过 resolve | lockfile 有效时完全消除 Resolve 阶段 | 已实现 | 继续保留并优先命中 fast path |

这里需要特别区分 **batch search API** 和 **batch metadata API**：

- npm registry 的 search API 主要用于搜索包，不提供 resolver 必需的完整版本列表、dist-tags、dependencies、tarball、integrity 等信息。
- resolver 需要的是 packument metadata。公开 npm registry 没有标准的“批量 packument”端点。
- 如果未来 orix 自建 registry proxy，可以设计私有 batch metadata endpoint；但这属于 registry proxy 能力，不应作为当前 resolver 快速优化的前提。

因此，当前文档中的“减少请求数”应优先理解为：

1. **single-flight 去重**：同一个 package name 同时只发一个 packument 请求。
2. **内存缓存**：同一次 install 内复用 packument。
3. **磁盘缓存**：跨进程、跨安装复用 packument。
4. **自建 proxy batch endpoint**：未来可选能力，而不是 npm registry 通用能力。

### 最快策略的实现边界

第一版只改：

| 文件 | 改动 |
| --- | --- |
| `crates/resolver/src/resolver.rs` | 并发化 `resolve_batch_impl`，移除 peer 同步 fetch |
| `crates/resolver/Cargo.toml` | 如需要，引入 `futures` 或改用 Tokio 原生集合 |
| `crates/core/src/pipeline.rs` | 创建 resolver 时传入 `concurrency` |
| `crates/resolver/src/resolver.rs` tests | 增加并发/去重/peer 不 fetch 的单元测试 |

第一版不改：

| 暂不处理 | 原因 |
| --- | --- |
| 磁盘 packument cache | 涉及 cache key、TTL、offline/force 语义，收益主要在重复安装 |
| ETag / cache-control | 需要 registry 协议细节和更多测试 |
| 版本索引缓存 | 并发化之前不是主瓶颈 |
| lockfile 增量解析 | 需要精确 closure 计算，风险高于收益 |
| 完整 peerDependencies | 属于 Phase 9 范围 |

### 最快策略验收

- mock registry 中每个 packument 固定延迟 100ms。
- 50 个唯一包、并发 10，Resolve 从约 5s 降到约 0.5s 到 1s。
- 同一 `(name, constraint)` 重复出现时只进入一次 resolution task。
- peer dependency 不触发额外 registry 请求。
- `cargo test -p orix-resolver` 和 `make check` 通过。

## 详细设计

### 一、并发解析调度器

#### 核心思路

将 resolver 从“递归/栈式串行解析”改为“有界并发工作队列”：

```text
root constraints
  -> enqueue tasks
  -> worker pool fetches packuments concurrently
  -> each completed task selects version and emits resolved package
  -> discovered dependencies enqueue new tasks
  -> until queue empty and in-flight count = 0
```

调度器需要维护：

| 状态 | 用途 |
| --- | --- |
| `pending` | 等待解析的 `(PackageName, VersionConstraint)` |
| `in_flight_resolution` | 已派发但未完成的 resolution key |
| `resolved_by_constraint` | `(name, raw constraint) -> PackageId` |
| `resolved_packages` | `PackageId -> ResolvedPackage`，用于图去重 |
| `in_flight_packuments` | `PackageName -> shared future/result`，去重 HTTP 请求 |
| `errors` | 非 optional 依赖失败立即终止；optional 依赖记录诊断 |

#### API 变更

最小变更是在 `Resolver` 上新增并发配置：

```rust
pub struct Resolver {
    registry: RegistryClient,
    memo: BTreeMap<(PackageName, String), PackageId>,
    resolve_concurrency: usize,
    skipped_optional: Vec<SkippedOptionalDep>,
    progress_tx: Option<mpsc::Sender<ResolveProgressEvent>>,
}

impl Resolver {
    pub fn with_concurrency(mut self, concurrency: usize) -> Self {
        self.resolve_concurrency = concurrency.max(1);
        self
    }
}
```

`core` 中创建 resolver 时传入现有 `config.concurrency`：

```rust
Resolver::new(config.registry.clone())
    .with_concurrency(concurrency)
    .with_progress(resolve_progress_tx)
```

后续如果发现 fetch concurrency 和 resolve concurrency 需要分离，再在 config 增加 `resolve-concurrency`。

#### 调度伪代码

```rust
async fn resolve_batch_concurrent(
    &mut self,
    graph: &mut DependencyGraph,
    roots: Vec<(PackageName, VersionConstraint)>,
) -> Result<()> {
    let semaphore = Arc::new(Semaphore::new(self.resolve_concurrency));
    let mut pending = VecDeque::from(roots);
    let mut in_flight = FuturesUnordered::new();

    loop {
        while in_flight.len() < self.resolve_concurrency {
            let Some((name, constraint)) = pending.pop_front() else {
                break;
            };

            let key = (name.clone(), constraint.raw.clone());
            if self.memo.contains_key(&key) || self.in_flight_resolution.contains(&key) {
                continue;
            }

            self.in_flight_resolution.insert(key.clone());

            let registry = self.registry.clone();
            let permit = semaphore.clone().acquire_owned().await?;

            in_flight.push(tokio::spawn(async move {
                let _permit = permit;
                resolve_one(registry, name, constraint).await
            }));
        }

        let Some(result) = in_flight.next().await else {
            break;
        };

        let resolved = result??;
        let deps = resolved.normal_and_optional_dependencies();

        self.memo.insert(resolved.constraint_key, resolved.id.clone());
        graph.insert(resolved.package);
        emit_progress();

        for dep in deps {
            pending.push_back(dep);
        }
    }

    Ok(())
}
```

实际实现中不一定需要 `tokio::spawn`；也可以用 `stream::iter(tasks).buffer_unordered(concurrency)`。由于依赖会动态发现，`FuturesUnordered` 更适合。

#### 顺序和确定性

并发完成顺序不稳定，但 lockfile 和 graph 输出必须稳定：

- `DependencyGraph` 继续使用 `BTreeMap<PackageId, ResolvedPackage>`，输出顺序稳定。
- progress 事件可以按完成顺序发出，不要求稳定。
- 错误信息中保留包名和约束即可，不依赖顺序。

### 二、in-flight packument 去重

#### 问题

并发后，多个 resolution task 可能同时需要同一个 package name。仅靠完成后的 cache 不够，会造成重复 HTTP 请求。

#### 设计

在 `RegistryClient` 或 resolver 内加入 single-flight 机制：

```rust
pub struct RegistryClient {
    cache: Arc<PackumentCache>,
    in_flight: Arc<Mutex<HashMap<PackageName, SharedPackumentFetch>>>,
}
```

更推荐放在 `crates/registry`，因为 packument 去重属于 registry 客户端能力，不应由 resolver 复制实现。

行为：

1. 先查内存缓存。
2. 缓存未命中时，尝试注册 in-flight fetch。
3. 如果已有同名 fetch，等待同一个结果。
4. 第一个请求完成后写入缓存并唤醒等待者。
5. 请求失败时移除 in-flight，允许后续重试。

可选实现方式：

- `tokio::sync::Mutex<HashMap<String, JoinHandle<Result<Packument>>>>`
- `futures::future::Shared<BoxFuture<'static, Result<Packument>>>`
- `tokio::sync::Notify` + 自定义 entry

为了避免新增复杂依赖，可以先用 `JoinSet` / `JoinHandle`，或在 resolver 层短期实现 request coalescing。

### 三、移除 peer 同步网络路径

#### MVP 行为调整

Resolve 主路径不再为每个 peer 立即网络查询。改为：

- 已在当前 graph / memo 中满足的 peer：记录为 satisfied。
- 未满足的 peer：记录 `ResolverDiagnostic::UnresolvedPeer` 或普通 warning。
- optional peer：静默或低优先级诊断。
- 不主动 `fetch_packument_sync`。

这符合当前设计中“MVP 不实现完整 peer 解析”的原则，也能避免 peer 依赖把主解析重新拖回串行。

#### 未来扩展点

完整 peer 算法阶段再引入独立的 peer resolution pass：

```text
normal dependency graph
  -> peer context assignment
  -> conflict diagnostics
  -> peer-aware package key generation
```

这个 pass 应该在主依赖图完成后执行，不能在每个包解析时阻塞网络。

### 四、磁盘 packument cache

#### 缓存位置

复用 config cache 根目录：

```text
~/.cache/orix/packuments/v1/
├── registry.npmjs.org/
│   ├── left-pad.json
│   └── @scope%2fpkg.json
```

Windows 使用同一逻辑路径，由 `dirs::cache_dir()` 决定根目录。

#### 缓存条目

```rust
struct CachedPackument {
    fetched_at_unix_ms: u64,
    registry: String,
    package: String,
    etag: Option<String>,
    cache_control: Option<String>,
    packument: Packument,
}
```

#### TTL 策略

短期先用固定 TTL：

- 默认 5 分钟，与内存 TTL 保持一致。
- 可通过 config 扩展为 `registry-cache-ttl`.
- `--force` 绕过磁盘缓存并刷新。
- `--offline` 只允许读取磁盘缓存；未命中时报错。

中期支持 HTTP 条件请求：

- 保存 `ETag`。
- 请求时带 `If-None-Match`。
- registry 返回 `304 Not Modified` 时刷新 `fetched_at`。

#### Auth token 安全

带 auth 的 registry 请求需要谨慎缓存：

- 缓存 key 必须包含 registry URL，不包含 token。
- 不保存 request headers。
- 私有 registry 的 packument 可能包含私有包元数据，因此缓存目录权限要尽量使用用户私有目录。
- 后续可以允许 `.npmrc` 配置关闭磁盘 packument cache。

### 五、版本索引缓存

#### 问题

同一个 packument 可能被多个 range 查询。每次都解析所有版本字符串会重复消耗 CPU。

#### 设计

在内存缓存中保存派生索引：

```rust
pub struct IndexedPackument {
    pub raw: Packument,
    pub sorted_versions: Vec<Version>,
}
```

版本选择变为：

```rust
fn select_version(indexed: &IndexedPackument, constraint: &VersionConstraint) -> Result<Version> {
    match &constraint.kind {
        Exact(v) => Ok(v.clone()),
        Range(range) => indexed
            .sorted_versions
            .iter()
            .rev()
            .find(|v| range.matches(v))
            .cloned()
            .context("no satisfying version"),
        Latest => ...
        Tag(tag) => ...
    }
}
```

索引只在 packument 进入缓存时构建一次。若不想改变 `registry` 类型，可以先在 resolver 内建立 `BTreeMap<PackageName, Vec<Version>>` 的临时索引。

### 六、lockfile 增量解析

现有 fast path 在 lockfile 与 manifest 完全匹配时可以跳过 resolver。下一步可以支持“部分变更”：

```text
old lockfile + new manifest
  -> direct deps 未变化的部分直接复用 package closure
  -> 新增/变更的 direct deps 进入 resolver
  -> 合并旧 closure 与新解析结果
```

第一阶段不建议直接做，因为需要准确计算 closure 和删除不再引用的包。可以作为并发解析稳定后的第二批优化。

## 进度事件设计

当前 progress 的 `total = pending.len() + resolved_count` 是运行估计值，在并发场景下会波动。建议改成两个概念：

```rust
pub struct ResolveProgressEvent {
    pub id: PackageId,
    pub resolved: usize,
    pub discovered: usize,
    pub in_flight: usize,
}
```

CLI 可以显示：

```text
Resolving dependencies 42/128 discovered
```

其中：

- `resolved`：已经完成并进入 graph 的包数。
- `discovered`：已经发现的唯一 resolution key 数。
- `in_flight`：当前网络/版本选择任务数。

为了兼容现有事件，也可以短期继续映射：

```rust
done = resolved
total = discovered
```

## 错误处理

### 非 optional dependency

任何非 optional 依赖发生以下错误时，Resolve 立即失败：

- package 404
- 网络请求超出重试次数
- 无满足版本
- registry 返回 metadata 缺少必要字段

并发任务中已经启动的请求可以自然完成并丢弃结果，不需要强行取消。

### optional dependency

optional dependency 失败时：

- 不加入 graph。
- 写入 `skipped_optional` 或 `graph.diagnostics`。
- 继续解析其他依赖。

平台不匹配应尽早跳过。若 package metadata 中 `os/cpu` 不兼容，并且该包来自 optional 依赖，则跳过并记录原因。

### peer dependency

peer dependency 不触发网络请求。未满足 peer 只产生诊断，不阻塞主解析。

## 测试计划

### 单元测试

- `select_version` 使用版本索引后仍选择最高满足版本。
- 同一 package name 多个 constraint 时，只 fetch 一次 packument。
- 同一 `(name, constraint)` 在 pending 和 in-flight 中重复出现时，只解析一次。
- peer dependency 未命中不调用 registry。
- optional dependency 失败不导致整个 graph 失败。

### 集成测试

使用本地 mock registry，不依赖真实 npm 网络：

```text
root
├── a -> c
├── b -> c
└── d -> e -> f
```

测试点：

- 并发度为 1 时行为与旧串行路径一致。
- 并发度为 4 时总请求数等于唯一 package name 数。
- graph 输出稳定，lockfile 序列化稳定。
- 进度事件最终 `resolved == graph.len()`。

### 性能基准

新增一个 resolver benchmark 或 xtask 子命令：

```bash
cargo xtask bench-resolver --fixture examples/basic-install --registry http://127.0.0.1:4873
```

建议记录：

| 指标 | 含义 |
| --- | --- |
| `resolve.total_ms` | Resolve 总耗时 |
| `registry.requests` | 实际 HTTP 请求数 |
| `registry.cache_hits.memory` | 内存缓存命中 |
| `registry.cache_hits.disk` | 磁盘缓存命中 |
| `resolver.packages_resolved` | graph 包数量 |
| `resolver.max_in_flight` | 峰值并发 |

mock registry 可以为每个 packument 固定延迟 100ms。这样串行 50 个包约 5s，并发 10 个包理论约 0.5s，可以稳定验证优化收益。

## 落地计划

### Phase 1：测量与保护网

- 增加 resolver mock registry 测试夹具。
- 增加 resolve 阶段 tracing 字段：package、constraint、cache_hit、duration_ms。
- 增加串行基线测试，锁定 graph 输出。
- 不改变功能行为。

### Phase 2：主循环并发化

- 为 `Resolver` 增加 `resolve_concurrency`。
- 将 `resolve_batch_impl` 改为有界并发 work queue。
- 保持 `DependencyGraph` 输出稳定。
- core 创建 resolver 时传入 config concurrency。

### Phase 3：single-flight packument

- 在 `RegistryClient` 内实现 in-flight request coalescing。
- 添加“同包并发请求只产生一次 HTTP 请求”的测试。
- 保留现有 TTL 内存缓存语义。

### Phase 4：peer 热路径瘦身

- 移除 `fetch_packument_sync` 在 resolver 主路径中的使用。
- peer 只做当前 graph / memo 检查和诊断。
- 如仍需保留 sync API，限制为测试或非 async 调用场景，resolver 不再调用。

### Phase 5：磁盘 packument cache

- 新增 `PackumentDiskCache`。
- 支持 `offline` / `force`。
- 先使用固定 TTL，再扩展 ETag。

### Phase 6：版本索引

- 对 packument 构建 sorted version index。
- range 查询复用索引。
- 增加热门包大版本列表的微基准。

## 风险与缓解

| 风险 | 缓解 |
| --- | --- |
| 并发导致 lockfile 输出不稳定 | graph 继续使用 `BTreeMap`；测试锁定序列化输出 |
| 重复依赖在 in-flight 状态下被重复解析 | 增加 `in_flight_resolution` set |
| registry 被过高并发压垮 | 默认并发 10；尊重 config；后续可按 registry host 限流 |
| peer 行为变化 | MVP 文档明确 peer 不强制解析；增加诊断测试 |
| 磁盘缓存污染私有 registry 元数据 | 按 registry URL 分目录；不保存 token；支持关闭缓存 |
| 并发错误难排查 | tracing 记录 task id、package、constraint、duration、cache hit |

## 推荐优先级

最高优先级是 Phase 1 到 Phase 4。这几步能直接解决“Resolve 阶段卡顿”的主要原因，而且不需要改变 lockfile 格式或完整 peer 算法。

磁盘缓存和版本索引属于第二批优化：它们对重复安装和大包版本选择很有帮助，但收益依赖使用场景。应在并发解析落地并有基准数据后再实现。

## 验收标准

- 使用 mock registry、每个 packument 延迟 100ms、50 个唯一包、并发 10 时，Resolve 耗时应接近 500ms 到 800ms，而不是约 5000ms。
- 同一 package name 被多个依赖引用时，HTTP packument 请求数为 1。
- `cargo test -p orix-resolver` 通过。
- `make check` 通过。
- `examples/basic-install` 首次安装 Resolve 阶段持续输出进度，且无长时间静默。

## 落地状态

以下 Phase 已完成实现（2026-05-19）：

### Phase 2: 并发解析调度器

已实现，代码在 `crates/resolver/src/resolver.rs`。

- `resolve_batch_concurrent` 使用 `tokio::task::JoinSet` 实现有界并发工作队列，默认并发 10。
- `ResolverState` 管理共享状态：graph、memo、in_flight 集合、discovered/resolved 计数。
- 依赖发现通过 `pending` VecDeque 动态追加新任务。
- 通过 `in_flight: HashSet<(PackageName, String)>` 在调度层面去重同一 `(name, constraint)` 重复派发。
- `resolve_concurrency` 可通过 `Resolver::with_concurrency()` 配置。

### Phase 3: registry HTTP 并发限制

已实现，代码在 `crates/registry/src/lib.rs`。

- `RegistryClient` 增加 `concurrency: Arc<Semaphore>` 字段。
- `fetch_packument` 在 HTTP 请求前 `acquire_owned()` permit。
- `with_concurrency(base_url, n)` 和 `with_auth_concurrency(base_url, token, n)` 构造函数。
- 单例内存 TTL 缓存（5分钟）保持不变。

### Phase 4: 移除 peer 同步网络查询

已实现，代码在 `crates/resolver/src/resolver.rs`。

- `resolve_peer_dep` 函数已删除。
- peer dependencies 不再触发 `fetch_packument_sync` 调用。
- peer 信息仍被解析并存入 `ResolvedPackage.peer_dependencies`，但不参与主动解析。
- `RegistryClient.fetch_packument_sync` 保留（为向后兼容及测试场景），但 resolver 不再调用。

### Phase 1: 进度事件更新

已实现。

- `ResolveProgressEvent` 字段从 `{id, index, total}` 改为 `{id, discovered, resolved}`。
- CLI `pipeline.rs` 中 `resolve_progress_forwarder` 已更新映射。

### 未完成 Phase

| Phase | 状态 | 原因 |
| --- | --- | --- |
| Phase 5: 磁盘 packument cache | 未开始 | 需要 cache key、TTL、offline/force 语义 |
| Phase 6: 版本索引缓存 | 未开始 | 并发化后 CPU 不是主瓶颈 |
