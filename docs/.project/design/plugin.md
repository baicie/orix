我认为最佳方案是：

> **核心能力用 Rust 内置 Hook 系统；用户插件用独立 Node Plugin Host 进程；通信走 JSON-RPC；插件通过 npm 包分发。**
> **不要用动态 Rust dylib，不要用 NAPI 作为插件主链路，WASM 可以作为后期安全插件形态。**

也就是：

```text
pnpm-rs Rust Core
    ↓
Plugin Manager
    ↓
Builtin Rust Plugins + External Node Plugin Host
    ↓
npm plugins: @pnpm-rs/plugin-xxx / @company/plugin-xxx
```

---

# 1. 为什么这个方案最好

你的项目是 **Rust 版包管理器 CLI**，插件体系要兼顾：

```text
1. Rust 核心性能
2. npm 生态可分发
3. 前端/Node 开发者容易写插件
4. 不破坏主安装流程
5. 不把核心设计绑死在 NAPI 上
6. 后续可扩展到 WASM / 企业插件 / 官方插件
```

所以几个方案对比：

| 方案                  | 优点                             | 缺点                                        | 结论                 |
| --------------------- | -------------------------------- | ------------------------------------------- | -------------------- |
| Rust dylib 动态库插件 | 性能高                           | ABI 不稳定，跨平台极麻烦，安全差            | 不推荐               |
| NAPI 插件             | 适合 Node 调 Rust                | 你的主程序是 Rust CLI，不适合作为插件主链路 | 不推荐做核心插件系统 |
| WASM 插件             | 安全、可沙箱                     | DX 差，Node/npm 生态接入麻烦                | 后期补充             |
| 外部进程插件          | 稳定、语言无关、隔离好           | 有 IPC 成本                                 | **最推荐**           |
| Node Plugin Host      | 前端开发者最容易写，npm 分发自然 | 需要管理 Node 进程                          | **最佳落地方案**     |

所以最佳实践是：

> **Rust Core 只定义 Hook 协议。插件运行在 Node Plugin Host 中。Rust 通过 JSON-RPC 调用插件。**

---

# 2. 总体架构

```text
pnpm-rs
├─ Rust CLI
│  ├─ 参数解析
│  ├─ 命令调度
│  └─ 退出码处理
│
├─ Rust Core
│  ├─ install
│  ├─ add
│  ├─ remove
│  ├─ run
│  ├─ resolver
│  ├─ fetcher
│  ├─ store
│  ├─ lockfile
│  └─ workspace
│
├─ Plugin Manager
│  ├─ Hook Registry
│  ├─ Builtin Rust Plugins
│  ├─ External Plugin Host Client
│  ├─ Plugin Resolver
│  ├─ Plugin Config Loader
│  └─ Permission / Timeout / Error Control
│
└─ Node Plugin Host
   ├─ 加载 npm 插件
   ├─ 执行 JS/TS 插件 hook
   ├─ JSON-RPC over stdio
   └─ 返回 hook 结果
```

运行链路：

```text
pnpm-rs install
   ↓
读取配置
   ↓
解析插件列表
   ↓
启动 Node Plugin Host
   ↓
加载插件
   ↓
install 流程中触发 hook
   ↓
插件返回修改结果
   ↓
Rust Core 继续执行
```

---

# 3. 插件类型设计

建议分三类。

## 1. Builtin Plugin

内置 Rust 插件。

适合：

```text
registry 基础逻辑
npmrc 解析
workspace 支持
engine 检查
lockfile 兼容
```

特点：

```text
快
稳定
无需用户安装
适合核心能力
```

---

## 2. External JS Plugin

通过 Node Plugin Host 运行的 npm 插件。

适合：

```text
换源策略
企业私有源规则
依赖审计
license 检查
node 版本管理
自定义安装策略
自定义命令
日志上报
```

这是最主要的用户插件形态。

---

## 3. WASM Plugin

后期支持。

适合：

```text
安全策略
纯计算插件
企业规则校验
不可信插件沙箱
```

MVP 不建议上来就做 WASM，先把 Hook 协议稳定下来。

---

# 4. 推荐目录结构

```text
pnpm-rs/
├─ crates/
│  ├─ pnpm-rs-cli/
│  ├─ pnpm-rs-core/
│  ├─ pnpm-rs-plugin/
│  │  ├─ src/
│  │  │  ├─ manager.rs
│  │  │  ├─ hook.rs
│  │  │  ├─ rpc.rs
│  │  │  ├─ resolver.rs
│  │  │  ├─ builtin.rs
│  │  │  └─ types.rs
│  ├─ pnpm-rs-resolver/
│  ├─ pnpm-rs-fetcher/
│  ├─ pnpm-rs-store/
│  └─ pnpm-rs-workspace/
│
├─ packages/
│  ├─ plugin-api/
│  │  ├─ src/
│  │  │  └─ index.ts
│  │  └─ package.json
│  │
│  ├─ plugin-host/
│  │  ├─ src/
│  │  │  ├─ index.ts
│  │  │  ├─ rpc.ts
│  │  │  ├─ loader.ts
│  │  │  └─ hooks.ts
│  │  └─ package.json
│  │
│  ├─ plugin-registry/
│  ├─ plugin-node-env/
│  └─ plugin-policy/
│
├─ npm/
│  ├─ main/
│  ├─ darwin-arm64/
│  ├─ linux-x64-gnu/
│  └─ win32-x64-msvc/
│
└─ pnpm-rs.config.mjs
```

---

# 5. 插件配置设计

支持两种方式。

## 方式一：`pnpm-rs.config.mjs`

推荐。

```js
import { defineConfig } from "@pnpm-rs/plugin-api";

export default defineConfig({
  plugins: [
    "@pnpm-rs/plugin-registry",

    [
      "@company/pnpm-rs-plugin-policy",
      {
        allowPrivateRegistryOnly: true,
        blockedPackages: ["left-pad"],
      },
    ],

    [
      "@pnpm-rs/plugin-node-env",
      {
        autoUse: true,
        strict: false,
      },
    ],
  ],
});
```

---

## 方式二：`package.json`

适合简单项目。

```json
{
  "pnpmRs": {
    "plugins": [
      "@pnpm-rs/plugin-registry",
      [
        "@pnpm-rs/plugin-node-env",
        {
          "autoUse": true
        }
      ]
    ]
  }
}
```

---

# 6. 插件安装方式

插件不要依赖项目 `node_modules`，否则会有启动悖论：

```text
install 之前 node_modules 还不存在
但插件又需要从 node_modules 加载
```

所以推荐设计一个全局插件仓库：

```text
~/.pnpm-rs/plugins/
```

命令：

```bash
pnpm-rs plugin add @pnpm-rs/plugin-registry
pnpm-rs plugin add @company/pnpm-rs-plugin-policy
pnpm-rs plugin list
pnpm-rs plugin remove @company/pnpm-rs-plugin-policy
pnpm-rs plugin sync
```

插件加载顺序：

```text
1. Builtin plugins
2. 本地路径插件，例如 ./tools/plugin.mjs
3. ~/.pnpm-rs/plugins 中的全局插件
4. 项目 node_modules 插件，可选支持
```

这样比较稳。

---

# 7. 插件 package.json 规范

例如：

`@company/pnpm-rs-plugin-policy/package.json`

```json
{
  "name": "@company/pnpm-rs-plugin-policy",
  "version": "0.1.0",
  "type": "module",
  "main": "./dist/index.js",
  "pnpmRsPlugin": {
    "apiVersion": "1",
    "name": "@company/pnpm-rs-plugin-policy",
    "hooks": [
      "config:resolved",
      "dependency:resolve",
      "install:plan",
      "fetch:before"
    ],
    "capabilities": {
      "fs": ["readProject"],
      "network": false,
      "process": false,
      "secrets": false
    }
  },
  "peerDependencies": {
    "@pnpm-rs/plugin-api": "^0.1.0"
  }
}
```

注意：

> JS 插件不是强沙箱。
> `capabilities` 主要用于权限声明、审计、减少敏感信息传递。
> 真正强安全插件，后面用 WASM 做。

---

# 8. Hook 设计

核心插件系统最重要的不是“能不能加载插件”，而是 **Hook 设计是否稳定**。

建议分这些阶段。

---

## 1. 配置阶段

```text
config:load
config:resolved
```

用途：

```text
修改 registry
注入默认配置
读取企业策略
处理 workspace 配置
```

示例：

```ts
async 'config:resolved'(ctx, config) {
  return {
    ...config,
    registry: 'https://registry.npmmirror.com'
  }
}
```

---

## 2. 依赖解析阶段

```text
dependency:beforeResolve
dependency:resolve
dependency:afterResolve
```

用途：

```text
替换包名
阻止某些包
强制版本
解析私有包
处理 alias
处理 workspace protocol
```

示例：

```ts
async 'dependency:beforeResolve'(ctx, request) {
  if (request.name === 'lodash') {
    return {
      ...request,
      range: '^4.17.21'
    }
  }

  return request
}
```

---

## 3. Registry 阶段

```text
registry:resolve
registry:beforeRequest
registry:afterResponse
```

用途：

```text
换源
scope registry
企业 token 注入
代理配置
请求日志
```

示例：

```ts
async 'registry:resolve'(ctx, input) {
  if (input.packageName.startsWith('@company/')) {
    return {
      registry: 'https://npm.company.com'
    }
  }

  return null
}
```

---

## 4. Fetch 阶段

```text
fetch:before
fetch:after
fetch:error
```

用途：

```text
自定义 tarball 下载
缓存命中
失败重试
下载限速
下载统计
```

---

## 5. 安装计划阶段

```text
install:beforePlan
install:plan
install:afterPlan
```

用途：

```text
审计依赖树
禁止某些 license
检查重复依赖
依赖治理
自动 dedupe
```

---

## 6. Lockfile 阶段

```text
lockfile:read
lockfile:write
lockfile:validate
```

用途：

```text
lockfile 兼容
企业字段注入
校验 lockfile 是否被篡改
```

---

## 7. Store 阶段

```text
store:beforePut
store:afterPut
store:gc
```

用途：

```text
包缓存策略
离线缓存
store 清理策略
```

---

## 8. Link 阶段

```text
link:before
link:after
```

用途：

```text
控制 node_modules 结构
处理软链接
处理 workspace link
```

---

## 9. Lifecycle 阶段

```text
lifecycle:beforeScript
lifecycle:afterScript
lifecycle:error
```

用途：

```text
拦截 postinstall
禁用危险脚本
注入环境变量
记录构建日志
```

---

## 10. Run 阶段

```text
run:before
run:after
run:error
```

用途：

```text
执行 npm scripts 前切 Node 版本
注入环境变量
收集命令耗时
```

---

## 11. 自定义命令

插件可以注册命令：

```text
pnpm-rs registry use npm
pnpm-rs env install 20
pnpm-rs audit-company
```

---

# 9. Hook 执行模型

Hook 分三种类型。

---

## 1. Waterfall Hook

一个插件的返回值会传给下一个插件。

适合：

```text
config:resolved
dependency:beforeResolve
install:plan
```

执行方式：

```text
input
  ↓ plugin A
result A
  ↓ plugin B
result B
  ↓ plugin C
result C
```

---

## 2. First Hook

第一个返回 `handled: true` 的插件接管结果。

适合：

```text
registry:resolve
dependency:resolve
fetch:before
```

示例：

```ts
return {
  handled: true,
  registry: "https://npm.company.com",
};
```

---

## 3. Event Hook

只通知，不允许修改核心数据。

适合：

```text
install:after
fetch:error
run:after
```

可以并发执行。

---

# 10. 插件顺序

建议：

```text
1. Builtin plugins
2. 用户配置中的 plugins，按数组顺序
3. workspace plugins
4. global plugins
```

插件顺序必须可预测。

例如：

```js
plugins: [
  "@company/policy",
  "@pnpm-rs/plugin-registry",
  "@pnpm-rs/plugin-node-env",
];
```

那就严格按这个顺序执行。

---

# 11. 插件 API 设计

`@pnpm-rs/plugin-api`

```ts
export interface PluginContext {
  cwd: string;
  projectRoot: string;
  workspaceRoot?: string;
  command: string;
  pnpmRsVersion: string;

  logger: {
    debug(...args: unknown[]): void;
    info(...args: unknown[]): void;
    warn(...args: unknown[]): void;
    error(...args: unknown[]): void;
  };

  cache: {
    get<T = unknown>(key: string): Promise<T | undefined>;
    set<T = unknown>(key: string, value: T): Promise<void>;
  };
}

export interface Plugin {
  name: string;
  setup(api: PluginApi, options?: unknown): void | Promise<void>;
}

export interface PluginApi {
  hooks: HookRegistry;
  commands: CommandRegistry;
}

export interface HookRegistry {
  tap<TInput, TOutput = TInput>(
    hookName: string,
    handler: (ctx: PluginContext, input: TInput) => Promise<TOutput> | TOutput,
  ): void;
}

export interface CommandRegistry {
  register(command: PluginCommand): void;
}

export interface PluginCommand {
  name: string;
  description?: string;
  run(
    ctx: PluginContext,
    args: string[],
  ): Promise<number | void> | number | void;
}

export function definePlugin(plugin: Plugin): Plugin {
  return plugin;
}

export function defineConfig(config: PnpmRsConfig): PnpmRsConfig {
  return config;
}

export interface PnpmRsConfig {
  plugins?: Array<string | [string, unknown]>;
}
```

---

# 12. 插件示例：换源插件

```ts
import { definePlugin } from "@pnpm-rs/plugin-api";

export default definePlugin({
  name: "@pnpm-rs/plugin-registry",

  setup(api, options) {
    const registryMap = {
      npm: "https://registry.npmjs.org/",
      npmmirror: "https://registry.npmmirror.com/",
    };

    api.hooks.tap("registry:resolve", async (ctx, input) => {
      const packageName = input.packageName;

      if (packageName.startsWith("@company/")) {
        return {
          handled: true,
          registry: "https://npm.company.com/",
        };
      }

      return {
        handled: true,
        registry: registryMap[options?.default || "npm"],
      };
    });

    api.commands.register({
      name: "registry:test",
      description: "Test registry latency",

      async run(ctx, args) {
        ctx.logger.info("Testing registries...");
      },
    });
  },
});
```

---

# 13. 插件示例：Node 版本管理插件

```ts
import { definePlugin } from "@pnpm-rs/plugin-api";

export default definePlugin({
  name: "@pnpm-rs/plugin-node-env",

  setup(api, options) {
    api.hooks.tap("run:before", async (ctx, input) => {
      const requiredNode = input.packageJson?.engines?.node;

      if (!requiredNode) {
        return input;
      }

      ctx.logger.info(`Project requires Node ${requiredNode}`);

      return {
        ...input,
        env: {
          ...input.env,
          PNPM_RS_NODE_ENGINE: requiredNode,
        },
      };
    });

    api.commands.register({
      name: "env",
      description: "Manage Node.js versions",

      async run(ctx, args) {
        const subCommand = args[0];

        if (subCommand === "current") {
          ctx.logger.info(process.version);
          return 0;
        }

        ctx.logger.warn(`Unknown env command: ${subCommand}`);
        return 1;
      },
    });
  },
});
```

---

# 14. Rust 侧 Plugin Manager 设计

核心类型：

```rust
pub enum HookKind {
    Waterfall,
    First,
    Event,
}

pub struct HookSpec {
    pub name: &'static str,
    pub kind: HookKind,
    pub timeout_ms: u64,
}

pub struct PluginManager {
    builtin_plugins: Vec<Box<dyn BuiltinPlugin>>,
    external_host: Option<PluginHostClient>,
}

impl PluginManager {
    pub async fn load(config: PluginConfig) -> anyhow::Result<Self> {
        // 1. load builtin plugins
        // 2. resolve external plugins
        // 3. start node plugin host when needed
        // 4. register hooks
        todo!()
    }

    pub async fn waterfall<T>(&self, hook: &str, input: T) -> anyhow::Result<T>
    where
        T: serde::Serialize + serde::de::DeserializeOwned,
    {
        // 1. run builtin hooks
        // 2. call external host
        // 3. return transformed input
        todo!()
    }

    pub async fn first<T, R>(&self, hook: &str, input: T) -> anyhow::Result<Option<R>>
    where
        T: serde::Serialize,
        R: serde::de::DeserializeOwned,
    {
        todo!()
    }

    pub async fn event<T>(&self, hook: &str, input: T) -> anyhow::Result<()>
    where
        T: serde::Serialize,
    {
        todo!()
    }
}
```

---

# 15. JSON-RPC 协议设计

Rust 启动 Node Plugin Host：

```text
node ~/.pnpm-rs/plugin-host/index.js
```

通信走 stdio。

## 初始化

Rust -> Host：

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "pluginHost/init",
  "params": {
    "cwd": "/repo/project",
    "workspaceRoot": "/repo",
    "pnpmRsVersion": "0.1.0",
    "plugins": [
      {
        "name": "@pnpm-rs/plugin-registry",
        "options": {
          "default": "npmmirror"
        }
      }
    ]
  }
}
```

Host -> Rust：

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "plugins": [
      {
        "name": "@pnpm-rs/plugin-registry",
        "hooks": ["registry:resolve"],
        "commands": ["registry:test"]
      }
    ]
  }
}
```

---

## 调用 Hook

Rust -> Host：

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "hook/call",
  "params": {
    "hook": "registry:resolve",
    "kind": "first",
    "input": {
      "packageName": "react",
      "scope": null
    }
  }
}
```

Host -> Rust：

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "result": {
    "handled": true,
    "value": {
      "registry": "https://registry.npmmirror.com/"
    }
  }
}
```

---

# 16. Node Plugin Host 设计

`packages/plugin-host/src/index.ts`

```ts
import { loadPlugins } from "./loader";
import { createRpcServer } from "./rpc";

const state = {
  plugins: [],
  hooks: new Map(),
};

const rpc = createRpcServer(process.stdin, process.stdout);

rpc.on("pluginHost/init", async (params) => {
  const result = await loadPlugins(params.plugins, {
    cwd: params.cwd,
    workspaceRoot: params.workspaceRoot,
    pnpmRsVersion: params.pnpmRsVersion,
  });

  state.plugins = result.plugins;
  state.hooks = result.hooks;

  return {
    plugins: result.plugins.map((plugin) => ({
      name: plugin.name,
      hooks: plugin.hooks,
      commands: plugin.commands,
    })),
  };
});

rpc.on("hook/call", async (params) => {
  const handlers = state.hooks.get(params.hook) || [];

  if (params.kind === "waterfall") {
    let current = params.input;

    for (const handler of handlers) {
      current = await handler(params.context, current);
    }

    return {
      value: current,
    };
  }

  if (params.kind === "first") {
    for (const handler of handlers) {
      const result = await handler(params.context, params.input);

      if (result && result.handled) {
        return {
          handled: true,
          value: result,
        };
      }
    }

    return {
      handled: false,
    };
  }

  if (params.kind === "event") {
    await Promise.allSettled(
      handlers.map((handler) => handler(params.context, params.input)),
    );

    return {};
  }
});
```

---

# 17. 插件错误处理

插件一定不能随便把主流程搞崩。

建议错误策略：

```text
config / resolve / install:plan 这类关键 hook：
  默认失败即失败

event / after / telemetry 这类 hook：
  默认失败只 warn，不中断

policy / security 类插件：
  可以显式声明 failureMode: "fail-closed"

日志插件：
  failureMode: "fail-open"
```

配置：

```js
export default defineConfig({
  plugins: [
    [
      "@company/pnpm-rs-plugin-policy",
      {
        failureMode: "fail-closed",
      },
    ],
    [
      "@company/pnpm-rs-plugin-logger",
      {
        failureMode: "fail-open",
      },
    ],
  ],
});
```

---

# 18. 超时控制

每个 hook 必须有超时。

默认：

```text
普通 hook: 10s
registry/fetch hook: 30s
event hook: 5s
```

配置：

```js
export default defineConfig({
  pluginTimeouts: {
    "registry:resolve": 5000,
    "install:plan": 30000,
  },
});
```

超时后：

```text
关键 hook：报错
非关键 hook：警告并跳过
```

---

# 19. 权限与安全

JS 插件不能当成强安全沙箱。

所以设计原则是：

```text
1. 插件默认视为可信代码
2. 不自动安装未知插件
3. 不在 install 时自动联网安装插件
4. 不向插件传递无关 token
5. token 只传 host/registry 匹配后的最小信息
6. 企业安全插件可以用 WASM 形态后续实现
```

可以增加首次启用提示：

```text
The project requires plugin "@company/pnpm-rs-plugin-policy".
This plugin can execute code on your machine.

Approve? [y/N]
```

记录到：

```text
~/.pnpm-rs/trusted-plugins.json
```

---

# 20. 插件缓存

为了性能，Node Plugin Host 不要每个 hook 都重新启动。

推荐：

```text
一次 pnpm-rs 命令生命周期内：
  只启动一个 Node Plugin Host

一次 install 流程内：
  所有 hook 复用这个进程

命令结束：
  关闭 plugin host
```

后续可以做 daemon：

```text
pnpm-rs daemon
```

但 MVP 不需要。

---

# 21. 性能优化

插件系统容易拖慢 install，所以要控制：

```text
1. 无插件时不启动 Node
2. 没有 external plugin 时只跑 Rust builtin hook
3. Hook 输入尽量小，不要传整个依赖树大对象
4. 大对象传 ID 或摘要
5. 高频 hook 慎重设计
6. registry 请求不要每个包都过 JS 插件，结果要缓存
```

例如 `registry:resolve` 可以缓存：

```text
@company/foo -> https://npm.company.com/
react        -> https://registry.npmjs.org/
```

---

# 22. Hook 数据不要太大

不要这样：

```text
每解析一个包都把完整 graph 发给插件
```

应该这样：

```text
dependency:resolve 只传当前依赖请求
install:plan 才传简化后的 plan summary
```

例如：

```ts
interface InstallPlanSummary {
  projectRoot: string;
  packages: Array<{
    name: string;
    version: string;
    integrity?: string;
    resolved?: string;
    dependencies?: Record<string, string>;
    dev?: boolean;
    optional?: boolean;
  }>;
}
```

---

# 23. 插件对核心数据的修改方式

不要让插件直接改内部结构。

建议使用：

```text
返回新对象
或返回 Patch
```

例如：

```ts
return {
  patches: [
    {
      op: "replace",
      path: "/dependencies/react/range",
      value: "^19.0.0",
    },
  ],
};
```

MVP 可以先用“返回新对象”，后期复杂结构用 Patch。

---

# 24. 插件命令设计

插件可以注册命令，但命令命名要避免冲突。

推荐格式：

```bash
pnpm-rs plugin <plugin-command>
pnpm-rs registry test
pnpm-rs env current
pnpm-rs audit-company
```

命令注册：

```ts
api.commands.register({
  name: "registry:test",
  description: "Test registry latency",
  async run(ctx, args) {
    // ...
  },
});
```

Rust CLI 收到未知命令时：

```text
先查 core command
再查 plugin command
```

---

# 25. 版本兼容策略

插件 API 要有版本。

```json
{
  "pnpmRsPlugin": {
    "apiVersion": "1"
  }
}
```

Rust Core 只加载兼容版本：

```text
pnpm-rs 0.1.x 支持 plugin api v1
pnpm-rs 0.2.x 支持 plugin api v1/v2
pnpm-rs 1.x 稳定 v1
```

插件加载失败提示：

```text
Plugin "@company/foo" requires pnpm-rs plugin API v2,
but current pnpm-rs supports v1.
```

---

# 26. 适合插件化的功能

## 适合插件

```text
换源策略
私有源认证
企业依赖治理
license 检查
安全审计
Node 版本管理
自定义 run 环境
自定义日志上报
lockfile 校验
monorepo 规则检查
```

---

## 不适合插件

```text
核心依赖解析算法
核心 lockfile 格式
核心 store 结构
核心 node_modules 链接算法
基础 npm registry 协议
```

这些应该在 Rust Core 里。

---

# 27. 针对你前面提到的“换源 / Node 版本管理”

我建议这样拆：

## 换源

内置基础能力：

```text
.npmrc
registry
@scope:registry
auth token
proxy
strict-ssl
```

插件只做增强：

```text
registry use npm
registry use npmmirror
registry test
企业源策略
自动选择最快源
```

---

## Node 版本管理

先做插件：

```text
@pnpm-rs/plugin-node-env
```

它提供：

```bash
pnpm-rs env current
pnpm-rs env install 20
pnpm-rs env use 20
pnpm-rs env list
```

并挂 hook：

```text
run:before
lifecycle:beforeScript
```

用于切换 Node 或注入环境变量。

---

# 28. MVP 插件系统建议只做这些

第一版不要做太大。

MVP 只做：

```text
1. Rust Hook Registry
2. Builtin Plugin 支持
3. Node Plugin Host
4. JSON-RPC stdio 通信
5. pnpm-rs.config.mjs
6. plugin add/list/remove/sync
7. 10 个左右核心 hook
8. 插件命令注册
9. 超时和错误策略
```

MVP Hook：

```text
config:resolved
registry:resolve
dependency:beforeResolve
dependency:resolve
fetch:before
fetch:after
install:beforePlan
install:plan
lifecycle:beforeScript
run:before
```

这已经够用了。

---

# 29. 最终推荐方案总结

你的项目最佳插件化设计是：

```text
Rust Core：
  提供稳定 Hook 点
  负责核心包管理逻辑
  默认无插件也能完整运行

Builtin Rust Plugins：
  承载高频、核心、性能敏感能力

Node Plugin Host：
  负责加载 npm 插件
  通过 JSON-RPC over stdio 和 Rust 通信
  插件按配置顺序执行

npm Plugin：
  使用 @pnpm-rs/plugin-api 编写
  支持 hooks + commands
  通过 pnpm-rs plugin add 安装到全局插件仓库

安全策略：
  JS 插件默认视为可信代码
  不自动安装未知插件
  不把 token 随便传给插件
  后期用 WASM 支持强沙箱插件
```

一句话：

> **核心 Hook 协议在 Rust，插件运行时在 Node，分发走 npm，安全插件未来走 WASM。**

这是最适合你这个 Rust 版 pnpm 项目的插件化路线。
