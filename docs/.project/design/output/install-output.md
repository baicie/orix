我觉得你现在这个日志**功能上已经能看懂**，但作为包管理器 CLI，默认输出还不够“高级”。主要问题是：

```txt
1. 重复打印了太多次 orix install 标题
2. Resolving / Fetching / Linking 每次刷新都进入终端历史，显得很吵
3. tracing 的 INFO 日志和用户 UI 混在一起了
4. Writing lockfile / Fetching packages / Linking packages 的展示顺序有点乱
5. 包列表反复出现，但用户真正关心的是最终结果、失败原因和耗时
```

我建议最终做成 **三套输出模式**：

```txt
默认模式：适合人看，简洁、漂亮
verbose 模式：适合调试，显示详细过程
CI / non-TTY 模式：适合日志系统，不用动画、不反复刷新
```

---

# 一、默认模式推荐格式

最终默认输出可以这样：

```txt
orix install
────────────────────────────────────────

Packages
  + is-even
  + left-pad

Registry
  https://registry.npmmirror.com/

Progress
  ✓ Resolved 6 packages
  ✓ Fetched 6 packages
  ✓ Linked dependencies
  ✓ Lockfile unchanged

Done in 0.39s
```

或者更像现代包管理器一点：

```txt
orix install
────────────────────────────────────────

Packages: +2 direct, +6 total
Registry: https://registry.npmmirror.com/

✓ Resolved dependencies
✓ Fetched packages 6/6
✓ Linked dependencies
✓ Lockfile unchanged

Done in 0.39s
```

我更推荐第二种，信息密度更好。

---

# 二、TTY 交互模式应该“原地刷新”，不要刷屏

现在你的输出像这样：

```txt
orix install
──────────────

Resolving dependencies

orix install
──────────────

Resolving dependencies
 → is-even
```

这会导致终端历史非常长。

更好的方式是：**TTY 模式下只维护一个动态区域，刷新同一块内容**。

运行中显示：

```txt
orix install
────────────────────────────────────────

Packages: +2 direct
Registry: https://registry.npmmirror.com/

✓ Resolved dependencies
◔ Fetching packages 4/6
  ✓ is-buffer
  ✓ is-even
  ✓ is-number
  ✓ is-odd

Next: linking dependencies
```

完成后再输出一个最终摘要：

```txt
orix install
────────────────────────────────────────

Packages: +2 direct, +6 total
Registry: https://registry.npmmirror.com/

✓ Resolved dependencies
✓ Fetched packages 6/6
✓ Linked dependencies
✓ Lockfile unchanged

Done in 0.39s
```

终端历史里最好只留下最终结果，不要留下每一次刷新。

---

# 三、不要默认输出 tracing 的 INFO

你现在有：

```txt
2026-05-17T14:55:06.312660Z  INFO orix_core::pipeline: using authenticated registry registry=https://registry.npmmirror.com/
```

这个对开发者有用，但对普通用户太底层。

默认模式建议改成：

```txt
Registry: https://registry.npmmirror.com/
```

详细日志放到：

```bash
orix install --verbose
```

或者：

```bash
ORIX_LOG=info orix install
ORIX_LOG=debug orix install
```

默认用户 UI 不应该混入：

```txt
orix_core::pipeline
duration_ms
INFO
timestamp
```

这些更适合 debug log。

---

# 四、推荐的阶段顺序

你的 install pipeline 展示可以固定成这几个阶段：

```txt
1. Resolving dependencies
2. Fetching packages
3. Linking dependencies
4. Running lifecycle scripts
5. Writing lockfile
6. Done
```

如果当前阶段没有发生，可以跳过。

例如没有 scripts，就不展示：

```txt
Running lifecycle scripts
```

最终摘要：

```txt
✓ Resolved 6 packages
✓ Fetched 6 packages
✓ Linked 6 packages
✓ Lockfile unchanged
```

如果 lockfile 有变化：

```txt
✓ Updated orix.lock
```

如果没有：

```txt
✓ Lockfile unchanged
```

---

# 五、默认输出最终建议

我建议你把默认输出定成这个样子：

```txt
orix install
────────────────────────────────────────

Packages: +2 direct, +6 total
Registry: https://registry.npmmirror.com/

✓ Resolved dependencies
✓ Fetched packages 6/6
✓ Linked dependencies
✓ Lockfile unchanged

Done in 0.39s
```

如果有缓存命中，可以这样：

```txt
orix install
────────────────────────────────────────

Packages: +2 direct, +6 total
Registry: https://registry.npmmirror.com/

✓ Resolved dependencies
✓ Fetched packages 6/6
  4 cached, 2 downloaded
✓ Linked dependencies
✓ Lockfile unchanged

Done in 0.39s
```

这个格式就很舒服。

---

# 六、Fetching 阶段不要一直显示全部包

如果包很多，默认不要把所有包都列出来。

### 少量包时

比如小于 10 个，可以展示：

```txt
◔ Fetching packages 4/6
  ✓ is-buffer
  ✓ is-even
  ✓ is-number
  ✓ is-odd
```

### 大量包时

比如超过 10 个，只展示最近几个：

```txt
◔ Fetching packages 124/388
  ✓ react
  ✓ scheduler
  ✓ loose-envify
  ✓ object-assign
  ...
```

完成后：

```txt
✓ Fetched packages 388/388
```

不要把 388 个包全刷出来，除非用户传：

```bash
orix install --verbose
```

---

# 七、错误格式建议

比如你之前出现过 integrity mismatch，错误最好这样展示：

```txt
orix install
────────────────────────────────────────

Packages: +2 direct, +6 total
Registry: https://registry.npmmirror.com/

✓ Resolved dependencies
✗ Failed to fetch left-pad@1.3.0

Integrity check failed

Expected:
  sha512-NcdALwpXkTm5Zvvbk7owOUSvVvBKDgKP5/ewfXEznmQFfs4ZRmanOeKBTjRVjka3QFoN6XJ+9F3USqfHqTaU5w==

Received:
  sha512-35c7402f0a579139b966fbdb93ba303944af56f04a0e028fe7f7b07d71339e64057ece194666a739e2814e34558e46b7405a0de9727ef45dd44aa7c7a93694e7

Package:
  left-pad@1.3.0

Hint:
  Try clearing the package cache and reinstalling:
  orix cache clean left-pad
  orix install
```

错误信息要做到：

```txt
什么失败了
哪个包失败了
为什么失败了
用户应该怎么处理
```

这比直接抛 Rust error 更像成熟工具。

---

# 八、CI / 非 TTY 模式格式

CI 里不要 spinner，不要 ANSI 动画，不要原地刷新。

比如 GitHub Actions 里应该这样：

```txt
orix install
packages: 2 direct, 6 total
registry: https://registry.npmmirror.com/

[1/4] resolving dependencies
[2/4] fetching packages 6/6
[3/4] linking dependencies
[4/4] writing lockfile

done in 0.39s
```

失败时：

```txt
[2/4] fetching packages
error: failed to fetch left-pad@1.3.0
reason: integrity mismatch
```

判断方式：

```txt
stdout 是 TTY：使用漂亮动态 UI
stdout 不是 TTY：使用普通 append-only 日志
--no-progress：强制普通日志
--verbose：显示详细日志
--json：输出机器可读事件
```

---

# 九、verbose 模式

```bash
orix install --verbose
```

可以输出：

```txt
orix install
────────────────────────────────────────

Registry:
  url: https://registry.npmmirror.com/
  auth: true

Resolution:
  is-even@latest -> 1.0.0
  left-pad@latest -> 1.3.0

Fetch:
  is-buffer@1.1.6 cached
  is-even@1.0.0 downloaded 12ms
  is-number@3.0.0 downloaded 8ms
  is-odd@0.1.2 cached
  kind-of@3.2.2 cached
  left-pad@1.3.0 downloaded 20ms

Link:
  node_modules/is-even -> .orix/store/is-even@1.0.0
  node_modules/left-pad -> .orix/store/left-pad@1.3.0

Lockfile:
  unchanged

Done in 0.39s
```

---

# 十、JSON 模式

后续可以给 CI / 插件系统留一个：

```bash
orix install --json
```

输出 NDJSON：

```json
{"type":"phase_start","phase":"resolve"}
{"type":"package_resolved","name":"is-even","version":"1.0.0"}
{"type":"phase_done","phase":"resolve","duration_ms":20}
{"type":"phase_start","phase":"fetch"}
{"type":"package_fetched","name":"left-pad","version":"1.3.0","cached":false}
{"type":"phase_done","phase":"fetch","total":6,"success":6,"failed":0}
{"type":"done","duration_ms":390}
```

这个以后做 VS Code 插件、agent 集成、CI 分析都很有用。

---

# 十一、最终我建议你采用的完整日志规范

## 默认成功

```txt
orix install
────────────────────────────────────────

Packages: +2 direct, +6 total
Registry: https://registry.npmmirror.com/

✓ Resolved dependencies
✓ Fetched packages 6/6
✓ Linked dependencies
✓ Lockfile unchanged

Done in 0.39s
```

## 默认安装中

```txt
orix install
────────────────────────────────────────

Packages: +2 direct
Registry: https://registry.npmmirror.com/

✓ Resolved dependencies
◔ Fetching packages 4/6
  ✓ is-buffer
  ✓ is-even
  ✓ is-number
  ✓ is-odd

Next: linking dependencies
```

## 默认失败

```txt
orix install
────────────────────────────────────────

Packages: +2 direct, +6 total
Registry: https://registry.npmmirror.com/

✓ Resolved dependencies
✗ Failed to fetch left-pad@1.3.0

Reason:
  Integrity check failed

Hint:
  Run:
  orix cache clean left-pad
  orix install
```

## CI 模式

```txt
orix install
packages: 2 direct, 6 total
registry: https://registry.npmmirror.com/

[1/4] resolving dependencies
[2/4] fetching packages 6/6
[3/4] linking dependencies
[4/4] writing lockfile

done in 0.39s
```

---

# 十二、代码架构建议

你可以不要在 pipeline 里直接 `println!`，而是抽象事件：

```rust
pub enum InstallEvent {
    Started {
        command: String,
    },
    RegistrySelected {
        url: String,
        authenticated: bool,
    },
    DependencyDiscovered {
        name: String,
    },
    PhaseStarted {
        phase: InstallPhase,
    },
    PackageFetched {
        name: String,
        version: String,
        cached: bool,
    },
    PhaseFinished {
        phase: InstallPhase,
        duration_ms: u64,
    },
    LockfileUnchanged,
    LockfileWritten,
    Finished {
        installed: usize,
        duration_ms: u64,
    },
    Failed {
        message: String,
        hint: Option<String>,
    },
}
```

然后渲染层分三种：

```rust
pub trait Reporter {
    fn on_event(&mut self, event: InstallEvent);
}
```

实现：

```txt
InteractiveReporter  // TTY 漂亮动态 UI
PlainReporter        // CI append-only 日志
JsonReporter         // NDJSON
```

这样后续不会乱。

---

# 最终结论

你现在的输出已经有雏形，但应该调整成：

```txt
默认：简洁摘要 + 动态进度
CI：无动画、逐行日志
verbose：详细包级日志
json：机器可读事件流
```

我最推荐的默认完成态就是：

```txt
orix install
────────────────────────────────────────

Packages: +2 direct, +6 total
Registry: https://registry.npmmirror.com/

✓ Resolved dependencies
✓ Fetched packages 6/6
✓ Linked dependencies
✓ Lockfile unchanged

Done in 0.39s
```

这比现在的多次重复刷新更清爽，也更像一个成熟包管理器。
