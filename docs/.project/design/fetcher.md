# Registry & Fetcher 设计 — 包下载管道

## 概述

`crates/registry` 负责与 npm registry API 通信（获取 packument、版本元数据）。`crates/fetcher` 负责实际的 tarball 下载、完整性验证和解压到暂存目录，供 store 消费。

## Registry API

### 使用的端点

| 操作 | URL | 方法 |
|------|-----|------|
| 获取 packument | `https://registry.npmjs.org/<package>` | GET |
| 获取 tarball | `https://registry.npmjs.org/<package>/-/<tarball>` | GET |

### Packument 获取

```rust
/// 获取一个包名的完整 packument
pub async fn fetch_packument(
    client: &Client,
    registry: &Url,
    name: &PackageName,
) -> Result<Packument> {
    let url = registry.join(&format!("{}/", name))?;
    let resp = client.get(url).send().await?;
    if resp.status() == StatusCode::NOT_FOUND {
        anyhow::bail!("package '{}' not found on registry", name);
    }
    let packument: Packument = resp.json().await?;
    Ok(packument)
}
```

### HTTP 客户端配置

```rust
fn make_client(config: &RegistryConfig) -> Client {
    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static("rpnpm/<version>"),
    );

    // 来自 .npmrc 的认证 token
    if let Some(token) = &config.auth_token {
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", token)).unwrap(),
        );
    }

    Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}
```

## Fetcher

### 核心职责

fetcher 接收一个 `ResolvedPackage`（来自 resolver）并生成一个包含解压后包文件的目录，准备好供 store 导入。

### Tarball 下载流程

```
1. 检查本地缓存（tarball URL → 缓存的 .tgz 路径）
2. 缓存未命中 → 从 tarball_url 下载 tarball
3. 验证完整性（registry 返回的 sha512 integrity 字符串）
4. 解压到临时目录
5. 验证解压根目录中存在 package.json
6. 返回解压后的目录路径
```

### 完整性验证

```rust
/// 根据 registry 返回的 integrity 字符串验证 tarball
pub async fn verify_tarball(tarball_path: &Path, expected_integrity: &str) -> Result<()> {
    let content = tokio::fs::read(tarball_path).await?;

    // 解析 integrity 字符串（可能是遗留 sha1 或现代 sha512）
    if expected_integrity.starts_with("sha512-") {
        let expected = &expected_integrity[7..];  // 去掉 "sha512-"
        let actual = sha512_digest(&content);
        if !secure_compare(actual.as_bytes(), expected.as_bytes()) {
            anyhow::bail!(
                "integrity mismatch: expected sha512-{}, got sha512-{}",
                expected_integrity,
                actual
            );
        }
    } else if expected_integrity.starts_with("sha1-") {
        let expected = &expected_integrity[5..];  // 去掉 "sha1-"
        let actual = sha1_digest(&content);
        if !secure_compare(actual.as_bytes(), expected.as_bytes()) {
            anyhow::bail!(
                "integrity mismatch: expected sha1-{}, got sha1-{}",
                expected_integrity,
                actual
            );
        }
    } else if expected_integrity.contains("-") {
        // 可能是多层 integrity 如 "sha512-xxx sha1-yyy"
        // 验证所有层级
    } else {
        // 无算法前缀的裸哈希（遗留格式）
        // 按 sha1 处理以保持向后兼容
    }

    Ok(())
}
```

**注意**：使用 `subtle::ConstantTimeEq` 进行常数时间比较，防止完整性检查受到时序攻击。

### Tarball 解压

```rust
/// 将 .tgz tarball 解压到目标目录
pub async fn extract_tarball(
    tarball_path: &Path,
    dest: &Path,
) -> Result<PathBuf> {
    let file = std::fs::File::open(tarball_path)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);

    // npm tarball 总是解压到 "package/" 子目录
    // 我们需要将其内容提升到 dest
    let mut entries = archive.entries()?;
    let mut first_entry_prefix = None;

    for entry in entries.by_ref() {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();

        // 跟踪第一个条目的前缀（通常是 "package/"）
        if first_entry_prefix.is_none() {
            first_entry_prefix = path.components().next().map(|c| c.as_os_str().to_owned());
        }

        // 如果存在 "package/" 前缀则剥离
        let stripped: PathBuf = path.components().skip(1).collect();
        let out_path = dest.join(&stripped);

        if let Some(parent) = out_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        entry.unpack(&out_path)?;
    }

    Ok(dest.to_path_buf())
}
```

## Tarball 缓存

tarball 在本地缓存以避免重复安装时重新下载：

```
~/.rpnpm/cache/tarballs/
├── sha256/<url-哈希>/
│   └── <package>-<version>.tgz
```

```rust
pub struct TarballCache {
    root: PathBuf,
    client: Client,
}

impl TarballCache {
    pub async fn get_or_fetch(
        &self,
        url: &Url,
        integrity: &str,
    ) -> Result<PathBuf> {
        let cache_key = sha256(url.as_str().as_bytes());
        let cached = self.root.join("sha256").join(&cache_key[..2])
            .join(&cache_key[2..])
            .join(format!("{}.tgz", cache_key));

        if cached.exists() {
            // 使用前验证完整性
            if Self::verify_tarball_sync(&cached, integrity).is_ok() {
                return Ok(cached);
            }
            // 缓存损坏 —— 重新下载
            tokio::fs::remove_file(&cached).await?;
        }

        // 下载
        let resp = self.client.get(url.clone()).send().await?;
        let bytes = resp.bytes().await?;
        tokio::fs::create_dir_all(cached.parent().unwrap()).await?;
        tokio::fs::write(&cached, &bytes).await?;

        // 写入后验证（覆盖下载了坏 tarball 的情况）
        Self::verify_tarball_sync(&cached, integrity)?;

        Ok(cached)
    }
}
```

## 并发下载策略

```rust
/// 并发下载并解压依赖图中的所有包
pub async fn fetch_all(
    graph: &DependencyGraph,
    cache: &TarballCache,
    store: &Store,
    concurrency: usize,
) -> Result<FetchReport> {
    let semaphore = Arc::new(Semaphore::new(concurrency));
    let client = Client::new();

    let tasks: Vec<_> = graph.packages.values()
        .map(|pkg| {
            let sem = semaphore.clone();
            let cache = cache.clone();
            let store = store.clone();
            let client = client.clone();

            tokio::spawn(async move {
                let _permit = sem.acquire().await?;
                let tarball = cache.get_or_fetch(&pkg.tarball_url, &pkg.integrity).await?;
                let temp_dir = tempfile::tempdir()?;
                let extracted = extract_tarball(&tarball, temp_dir.path()).await?;
                store.import_package(&pkg.id, extracted).await?;
                Ok::<_, anyhow::Error>(())
            })
        })
        .collect();

    let results = futures::future::join_all(tasks).await;
    let mut report = FetchReport::default();

    for result in results {
        match result {
            Ok(Ok(())) => report.success += 1,
            Ok(Err(e)) => { report.failures.push(e.to_string()); }
            Err(e) => { report.failures.push(e.to_string()); }
        }
    }

    Ok(report)
}
```

10 的并发限制防止压垮 registry，同时对大型依赖树仍保持足够的速度。

## 错误处理

| 错误 | 原因 | 恢复方式 |
|------|------|----------|
| `HttpError(404)` | 包不在 registry 上 | 提前失败，消息清晰 |
| `IntegrityMismatch` | tarball 损坏或被篡改 | 删除缓存，重试下载 |
| `Timeout` | 网络慢 | 指数退避重试 |
| `DiskFull` | tarball 无空间存放 | 失败并给出清晰错误 |
