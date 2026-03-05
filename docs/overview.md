# distribution-rs 重写规划

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 以 Rust 对 github.com/distribution/distribution 做 feature-parity 翻译式重写，通过 OCI Distribution Spec conformance test。

**Architecture:** 保持与 Go 版本相同的模块边界，但拆除 God-object `App`，改用 `Arc<AppState>` + axum 的依赖注入。`StorageDriver` 和 `ManifestService` 等核心接口映射为 async trait，`Manifest` 类型从 Go 的 `interface{}` 改为 Rust enum（关闭集合）。

**Tech Stack:** tokio 1.x, axum 0.8.x, async-trait 0.1.x, sha2 0.10.x, serde 1.x, jsonwebtoken 9.x, figment 0.10.x, object_store 0.11.x（仅作云存储 driver 内部实现），prometheus 0.13.x, tracing + opentelemetry 0.27.x

---

## 第一步：Go 架构考古

以下内容来自对 `/Users/zhangbin/Arcbox/distribution/` 源码的直接阅读，均可追溯到具体文件。

### 1.1 顶层模块划分

| 目录 | 职责 |
|------|------|
| `cmd/registry/` | 入口，通过 blank import 触发所有 driver/auth 的 `init()` 注册 |
| `configuration/` | YAML 配置解析（`configuration.go`），包含 Storage、HTTP、Auth、Notifications 等子结构 |
| `registry/handlers/` | HTTP 请求处理：`app.go`（路由+中间件+God-object App）、`manifests.go`、`blob.go`、`blobupload.go` |
| `registry/auth/` | `auth.go` 定义 `AccessController` interface；子目录 `token/`（JWT）、`htpasswd/`、`silly/` |
| `registry/storage/` | `ManifestService`、`BlobStore`、`TagService` 的实现；`linkedblobstore.go`（仓库隔离）、`blobwriter.go`（上传状态机） |
| `registry/storage/driver/` | `StorageDriver` interface；`filesystem/`、`s3-aws/`、`gcs/`、`azure/`、`inmemory/` |
| `registry/storage/cache/` | BlobDescriptorService 缓存（in-memory LRU + Redis） |
| `registry/middleware/` | storage/registry/repository 三级中间件插槽 |
| `registry/proxy/` | 透明代理（pull-through cache） |
| `manifest/` | Schema2、OCI Manifest、ManifestList 类型定义及全局注册表（`RegisterManifestSchema`） |
| `reference/` | 镜像引用解析（vendored `distribution/reference`） |
| `notifications/` | 事件系统：`event.go`、`bridge.go`、`sinks.go`（异步队列）、`endpoint.go`（HTTP webhook） |
| `health/` | 健康检查框架 |
| `tracing/` | OpenTelemetry 集成 |
| `blobs.go` / `manifests.go` / `registry.go` | 顶层接口定义（package distribution） |

### 1.2 请求处理链路

**docker push 的 blob 上传（三步）：**

```
POST /v2/{name}/blobs/uploads/
  → app.dispatcher() [handlers/app.go:699]
    → app.authorized() → auth.AccessController.Authorized()
    → app.registry.Repository(ctx, name) → distribution.Repository
    → notifications.Listen(repo, ...) 包装 repo
    → blobUploadDispatcher() [handlers/blobupload.go:21]
      → blobUploadHandler.StartBlobUpload()
        → linkedBlobStore.Create() [storage/linkedblobstore.go:128]
          → 生成 UUID，写 startedat 到 storage
          → 返回 blobWriter [storage/blobwriter.go:28]
        → 响应 202, Location: /v2/{name}/blobs/uploads/{uuid}?_state=<HMAC token>

PATCH /v2/{name}/blobs/uploads/{uuid}
  → 解析 _state HMAC token [handlers/hmac.go]
  → PatchBlobData() → copyFullPayload() → blobWriter.Write()
    → fileWriter.Write() → storage driver
    → sha256 digester 持续更新

PUT /v2/{name}/blobs/uploads/{uuid}?digest=sha256:xxx
  → PutBlobUploadComplete() → blobWriter.Commit()
    → fileWriter.Commit() → storage driver 落盘
    → validateBlob(): 校验 size + digest
    → moveBlob(): temp path → /blobs/sha256/{prefix}/{hash}/data
    → linkBlob(): 在 _layers/ 创建 link 文件
    → 响应 201, Location: /v2/{name}/blobs/sha256:xxx
```

**docker pull 的 blob 读取：**

```
GET /v2/{name}/blobs/{digest}
  → blobDispatcher → blobHandler.GetBlob() [handlers/blob.go:55]
    → linkedBlobStore.Stat() → 检查 _layers/ link 存在（仓库隔离）
    → linkedBlobStore.ServeBlob()
      → blobServer.ServeBlob() [storage/blobserver.go:26]
        → driver.RedirectURL() → 云存储返回预签名 URL → 307 Redirect
        → 无 redirect: newFileReader() + http.ServeContent()（处理 Range 请求）
```

**docker push manifest：**

```
PUT /v2/{name}/manifests/{reference}
  → manifestDispatcher → PutManifest() [handlers/manifests.go:241]
    → copyFullPayload() 读取 HTTP body
    → distribution.UnmarshalManifest() 按 Content-Type 路由到对应 handler
    → manifests.Put() → manifestStore.Put()
      → 对应 schema2Handler/ocischemaHandler.Put()
        → linkedBlobStore 存储 manifest 内容为 blob
        → 写 revisions/ link 文件
    → tags.Tag() 更新 tag link
    → 响应 201
```

### 1.3 核心接口

**StorageDriver** (`registry/storage/driver/storagedriver.go`)：
```go
type StorageDriver interface {
    Name() string
    GetContent(ctx, path string) ([]byte, error)
    PutContent(ctx, path string, content []byte) error
    Reader(ctx, path string, offset int64) (io.ReadCloser, error)
    Writer(ctx, path string, append bool) (FileWriter, error)
    Stat(ctx, path string) (FileInfo, error)
    List(ctx, path string) ([]string, error)
    Move(ctx, sourcePath, destPath string) error
    Delete(ctx, path string) error
    RedirectURL(r *http.Request, path string) (string, error)
    Walk(ctx, path string, f WalkFn, options ...func(*WalkOptions)) error
}
```

**ManifestService** (`manifests.go`)：
```go
type ManifestService interface {
    Exists(ctx, dgst digest.Digest) (bool, error)
    Get(ctx, dgst, options...) (Manifest, error)
    Put(ctx, manifest, options...) (digest.Digest, error)
    Delete(ctx, dgst) error
}
```

**BlobStore** (`blobs.go`)：由 BlobStatter + BlobProvider + BlobIngester + BlobServer + BlobDeleter 组合。
注意：`BlobServer.ServeBlob` 直接接收 `http.ResponseWriter`，是存储层耦合 HTTP 的反模式，Rust 重写中需纠正。

**AccessController** (`registry/auth/auth.go`)：
```go
type AccessController interface {
    Authorized(r *http.Request, access ...Access) (*Grant, error)
}
```

### 1.4 关键耦合点

| 耦合点 | Go 源文件 | Rust 处理方案 |
|--------|----------|--------------|
| `BlobServer` 直接持有 `http.ResponseWriter` | `blobs.go`, `blobserver.go` | 从 BlobStore trait 中移除，逻辑上移到 HTTP handler |
| `App` struct 是 God-object | `handlers/app.go` | 拆分为 `Arc<AppState>` + 显式 builder |
| `context.Context` 同时承担取消传播和键值存储 | 全局 | 取消用 `CancellationToken`，键值改为显式 `RequestContext` struct |
| `init()` 全局注册（driver/auth/manifest schema） | `cmd/registry/main.go` + 各 driver | 用显式注册函数，避免全局可变状态 |
| manifest schema 的 `UnmarshalFunc` 全局注册表 | `manifest/*.go` | 改为 enum 匹配（类型集合已知且封闭） |
| HMAC state token 编码 upload 状态 | `handlers/hmac.go` | 保持兼容格式（JSON + HMAC-SHA256 + base64url） |

---

## 第二步：Rust 重写规划

### 架构总览

OCI Distribution Registry (Rust) 模块依赖图：

```
┌─────────────────────────────────────────────────────────────┐
│                    bin/registry (main.rs)                    │
│          clap CLI → load config → build AppState            │
└──────────────────────────┬──────────────────────────────────┘
                           │ Arc<AppState>
┌──────────────────────────▼──────────────────────────────────┐
│                registry-http (axum 0.8)                      │
│  Router: /v2/* routes                                        │
│  Middleware: tracing / auth / read-only guard               │
│  Handlers: base / blob / blob_upload / manifest / tags       │
│            catalog / health / metrics                        │
└─────┬────────────┬──────────────────┬───────────────────────┘
      │            │                  │
      ▼            ▼                  ▼
registry-auth  registry-notifications  registry-config
(token/htpasswd) (mpsc + reqwest)      (figment + serde_yaml)
      │
      ▼
┌─────────────────────────────────────────────────────────────┐
│               registry-storage (core logic)                  │
│  ManifestStore / BlobStore / TagStore / LinkedBlobStore      │
│  BlobWriter (upload state machine)                           │
│  Paths (path layout spec)                                    │
│  Cache (LRU / Redis)                                         │
└──────────────────────────┬──────────────────────────────────┘
                           │ dyn StorageDriver
       ┌───────────────────┼───────────────────┐
       ▼                   ▼                   ▼
  registry-           registry-           registry-
  storage-driver-fs   storage-driver-s3   storage-driver-gcs
  (tokio::fs)         (object_store)      (object_store)
```

### 模块清单

| 模块名 | Go 对应包 | 核心职责 | 关键 Rust crate |
|--------|-----------|----------|----------------|
| `registry-core` | `blobs.go`, `manifests.go`, `registry.go` | 顶层 trait 定义、Digest/Descriptor/ManifestBody 类型、OCI 错误码 | `serde 1.x`, `thiserror 2.x`, `sha2 0.10.x`, `bytes 1.x` |
| `registry-reference` | `vendor/distribution/reference/` | 镜像引用解析（Named/Tagged/Digested） | `regex 1.x` |
| `registry-storage-driver` | `registry/storage/driver/` | `StorageDriver` trait + `FileWriter` trait + FileInfo | `async-trait 0.1.x`, `tokio 1.x` |
| `registry-storage-driver-fs` | `registry/storage/driver/filesystem/` | 本地文件系统实现（原子写入） | `tokio::fs`, `tempfile 3.x` |
| `registry-storage-driver-s3` | `registry/storage/driver/s3-aws/` | AWS S3（内部用 object_store） | `object_store 0.11.x` |
| `registry-storage-driver-gcs` | `registry/storage/driver/gcs/` | Google Cloud Storage | `object_store 0.11.x` |
| `registry-storage-driver-azure` | `registry/storage/driver/azure/` | Azure Blob Storage | `object_store 0.11.x` |
| `registry-storage` | `registry/storage/` | BlobWriter 状态机、ManifestStore、LinkedBlobStore、TagStore、路径布局、GC | `lru 0.12.x`, `uuid 1.x`, `hmac 0.12.x` |
| `registry-auth` | `registry/auth/` | `AccessController` trait、token JWT 验证、htpasswd | `jsonwebtoken 9.x`, `bcrypt 0.15.x` |
| `registry-notifications` | `notifications/` | 事件系统（Event、Bridge、Sink、HTTP endpoint） | `reqwest 0.12.x`, `tokio::sync::mpsc` |
| `registry-config` | `configuration/` | YAML 配置加载、环境变量展开 | `figment 0.10.x`, `serde_yaml 0.9.x` |
| `registry-health` | `health/` | 健康检查框架 | `axum 0.8.x` |
| `registry-http` | `registry/handlers/` | axum Router、AppState、所有 HTTP handler、中间件 | `axum 0.8.x`, `tower 0.5.x`, `tower-http 0.6.x` |
| `bin/registry` | `cmd/registry/` | CLI 入口、装配所有组件 | `clap 4.x`, `tokio 1.x` |

---

## 分阶段规划

### Phase 0：基础设施

**目标：** 建立 Rust workspace，实现所有核心类型（Digest、Reference、OCI 错误码、Descriptor、ManifestBody），无 I/O、无网络，纯数据层。后续所有 crate 依赖本 phase 输出。

**包含模块：** `registry-core`、`registry-reference`

**Workspace 目录结构：**
```
oci-registry/
├── Cargo.toml                    # workspace manifest
└── crates/
    ├── registry-core/
    │   └── src/
    │       ├── lib.rs
    │       ├── digest.rs         # Digest, Algorithm, Digester
    │       ├── descriptor.rs     # OCI Descriptor
    │       ├── manifest/
    │       │   ├── mod.rs
    │       │   ├── schema2.rs    # Docker Image Manifest V2
    │       │   ├── oci.rs        # OCI Image Manifest
    │       │   └── index.rs      # OCI Index + Docker Manifest List
    │       ├── error.rs          # OciErrorCode, OciErrors
    │       └── content_type.rs   # media type 常量
    └── registry-reference/
        └── src/
            ├── lib.rs
            ├── regexp.rs         # 移植自 distribution/reference 的正则
            ├── named.rs          # Named/Tagged/Digested traits
            └── normalize.rs      # docker.io/library 规范化
```

**关键决策点：**

1. **Digest 类型**：自行实现（`struct Digest { algorithm: DigestAlgorithm, encoded: String }`），不依赖第三方 digest crate（`opencontainers-digest` 维护状态不佳）。用 `sha2 0.10.x` + `hex 0.4.x` 实现 `Digester`。
2. **ManifestBody 为 enum**：Schema2 / OCI Manifest / DockerManifestList / OCI Index 四个 variant。类型集合封闭，编译期穷举，避免 `dyn Manifest` 的堆分配和 downcast。
3. **Reference parsing**：直接从 Go 的 `distribution/reference` 移植正则 grammar，使用 `regex 1.x`。
4. **OCI 错误码**：`enum OciErrorCode`（BLOB_UNKNOWN, DIGEST_INVALID 等）+ `struct OciErrors { errors: Vec<OciError> }` 可序列化为 OCI 规范 JSON。

**完成标准：**
```bash
cargo test -p registry-core
# 必须通过：
# - Digest::parse("sha256:abc...") == Ok(...)
# - Digest::parse("sha256:") == Err(DigestInvalid)
# - digest_path_components("sha256:aabb...") == ["sha256", "aa", "aabb..."]
# - Reference::parse("library/ubuntu:22.04") 返回 Named+Tagged
# - Reference::parse("ubuntu@sha256:abc...") 返回 Named+Digested
# - OciErrors 序列化输出与 OCI spec JSON 格式一致
cargo test -p registry-reference
```

---

### Phase 1：存储层

**目标：** 实现 `StorageDriver` trait 及文件系统 driver，实现 blob 上传状态机（POST/PATCH/PUT 三步流程），建立路径布局规范。通过 driver compliance test suite。

**包含模块：** `registry-storage-driver`、`registry-storage-driver-fs`、`registry-storage`（paths、blob_writer、blob_store、linked_blob_store、manifest_store、tag_store）

**目录结构：**
```
crates/
├── registry-storage-driver/
│   └── src/
│       ├── lib.rs                # StorageDriver trait（10 个方法）
│       ├── error.rs
│       └── walk.rs               # WalkFn, WalkOptions
├── registry-storage-driver-fs/
│   └── src/
│       ├── lib.rs                # FilesystemDriver: StorageDriver
│       ├── file_writer.rs        # FsFileWriter（原子写入 via rename）
│       └── tests.rs              # driver compliance test suite
└── registry-storage/
    └── src/
        ├── lib.rs
        ├── paths.rs              # 所有 pathSpec variants（精确对应 paths.go）
        ├── blob_store.rs         # 内容寻址 blob 读/stat
        ├── blob_writer.rs        # 可恢复上传状态机
        ├── linked_blob_store.rs  # 仓库隔离 blob 访问
        ├── manifest_store.rs     # Manifest CRUD + handler dispatch
        ├── tag_store.rs          # tag → digest 映射
        ├── registry.rs           # Registry + Repository struct
        └── vacuum.rs             # GC / 孤立 blob 清理
```

**关键决策点：**

1. **StorageDriver trait 用 `async-trait` 宏**（0.1.83+）：使 `dyn StorageDriver + Send + Sync` 对象安全。AFIT（Rust 1.75+）在 `dyn` 下仍有对象安全限制，`async-trait` 绕过此问题。
2. **FileWriter 原子写入**：写入 UUID 命名临时文件，`commit()` 调用 `tokio::fs::rename`（POSIX 原子），`cancel()` 删除临时文件。
3. **walk() 返回 Stream**：替换 Go 的回调 `WalkFn`，返回 `BoxStream<'_, Result<FileInfo, StorageError>>`（`futures 0.3.x`），更符合 Rust 组合风格。
4. **BlobServer 从存储层移除**：Go `BlobStore` 包含 `ServeBlob(w http.ResponseWriter, ...)` 是反模式。Rust 版本 blob 服务逻辑在 HTTP handler 层（调用 `BlobProvider::open()` + 手动 Range 处理）。
5. **可恢复 digest 状态**：Go 用 `encoding.BinaryMarshaler` 将 SHA256 hasher 内部状态持久化到 `_uploads/{uuid}/hashstates/sha256/{offset}`。`sha2 0.10.x` 不暴露内部状态序列化 API。**Phase 1 决定：实现非可恢复模式**（对应 Go 的 `blobwriter_nonresumable.go`），PATCH 续传时重新读已写数据重算 SHA256。标注 `// TODO(resumable-digest)`，Phase 3 后评估。
6. **paths.rs 精确对应 paths.go**：路径布局必须与 Go 实现 byte-for-byte 兼容（保证可读取现有 Go registry 数据）。每个 pathSpec variant 写单元测试验证路径字符串完全匹配。

**完成标准：**
```bash
# StorageDriver compliance suite（移植自 registry/storage/driver/testsuites/）
cargo test -p registry-storage-driver-fs -- --test-threads=1
# 覆盖：GetContent, PutContent, Reader, Writer (append/commit/cancel), Stat, List, Move, Delete, Walk

# 路径布局单元测试
cargo test -p registry-storage paths::tests
# 验证每个 pathSpec 输出字符串与 Go paths.go 完全一致

# Blob upload 状态机
cargo test -p registry-storage blob_writer::tests::test_commit_moves_to_content_addressed_path
cargo test -p registry-storage blob_writer::tests::test_cancel_removes_upload_dir
cargo test -p registry-storage blob_writer::tests::test_digest_mismatch_returns_error
```

**关键 Go 参考文件：**
- `registry/storage/driver/storagedriver.go` — trait 定义来源
- `registry/storage/paths.go` — 路径布局规范（**必须精确对应**）
- `registry/storage/blobwriter.go` — `Commit`/`Cancel`/`validateBlob`/`moveBlob` 逻辑
- `registry/storage/driver/filesystem/driver.go` — 文件系统实现参考

---

### Phase 2：Registry 核心逻辑

**目标：** 实现 `ManifestService`、`BlobStore`（含 `LinkedBlobStore`）、`TagService`，构建完整 namespace/repository 层级，支持通过存储层 API 直接完成 push/pull（不经过 HTTP）。

**包含模块：** `registry-storage`（registry.rs、blob_access_controller.rs、cache/memory.rs、gc.rs）、`registry-integration-tests`（新建）

**关键决策点：**

1. **ManifestHandler dispatch**：Go 的 `manifestStore` 持有四个 handler（按 media type 分发）。Rust 版本用 `HashMap<&'static str, Box<dyn ManifestHandler>>`，PUT 时按 `Content-Type` 选 handler，GET 时按存储 JSON 自动检测类型。
2. **LinkedBlobStore 仓库隔离**：`BlobProvider::get()` 前先检查 `_layers/{repo}/sha256/{hash}/link` 存在，不存在返回 `BlobUnknown`。这是安全边界，不可省略。
3. **跨仓库 blob mounting**：`BlobCreateOption::MountFrom { source_repo, digest }` — 检查源仓库有 link，若有则直接在目标仓库创建 link，返回 `Mounted` 信号。
4. **Descriptor 缓存**：接入 in-memory LRU 缓存（`lru 0.12.x`）包装 `BlobDescriptorService`，热路径 `stat()` 不走 storage driver。
5. **Arc<dyn StorageDriver> 而非泛型**：registry/repository 结构体始终用 `Arc<dyn StorageDriver>`，避免泛型爆炸。

**完成标准：**
```bash
# 集成测试（tmpdir + filesystem driver，通过存储 API 完成完整流程）
cargo test -p registry-integration-tests
# 必须通过：
# 1. 上传 blob（三步状态机直接调用）→ 按 digest 拉取
# 2. 上传 manifest（引用已存在 blob）→ 按 digest 拉取 → 按 tag 拉取
# 3. 列出 tags
# 4. 跨仓库 blob mount（mount 后可直接拉取）
# 5. GC 运行后孤立 blob 被清理，linked blob 保留
```

---

### Phase 3：HTTP API 层

**目标：** 实现 axum HTTP 服务器，通过 OCI Distribution Spec conformance test 的 Basic / Push / Pull / Content Discovery 四类测试。

**包含模块：** `registry-http`、`bin/registry`（无 auth 的简单配置）

**目录结构：**
```
crates/registry-http/
└── src/
    ├── lib.rs
    ├── app.rs            # AppState struct（替代 Go App god-object）
    ├── router.rs         # build_router() → axum::Router
    ├── errors.rs         # OciErrors → JSON response
    ├── extractors.rs     # name/reference/uuid 路径提取器
    ├── hmac.rs           # HMAC state token（精确移植 handlers/hmac.go）
    └── handlers/
        ├── base.rs       # GET /v2/
        ├── blob.rs       # GET|HEAD /v2/{name}/blobs/{digest}
        ├── blob_upload.rs # POST/PATCH/PUT/DELETE /v2/{name}/blobs/uploads/
        ├── manifest.rs   # GET|HEAD|PUT|DELETE /v2/{name}/manifests/{reference}
        ├── tags.rs       # GET /v2/{name}/tags/list
        └── catalog.rs    # GET /v2/_catalog
```

**关键决策点：**

1. **AppState**：
   ```rust
   #[derive(Clone)]
   pub struct AppState {
       pub config: Arc<Configuration>,
       pub registry: Arc<dyn Namespace>,
       pub access_controller: Arc<dyn AccessController>,
       pub notification_sender: mpsc::UnboundedSender<NotificationEvent>,
       pub read_only: bool,
   }
   ```
   所有字段 `Arc` 包裹，`AppState: Clone + Send + Sync`，axum 通过 `State<AppState>` 提取。

2. **完整路由表**（对应 Go 的 `v2.RouteNameXxx` 常量）：
   - `GET /v2/` — version check
   - `HEAD|GET /v2/{name}/blobs/{digest}` — blob get
   - `DELETE /v2/{name}/blobs/{digest}` — blob delete
   - `POST /v2/{name}/blobs/uploads/` — start upload
   - `GET|HEAD /v2/{name}/blobs/uploads/{uuid}` — upload status
   - `PATCH /v2/{name}/blobs/uploads/{uuid}` — patch data
   - `PUT /v2/{name}/blobs/uploads/{uuid}` — complete upload
   - `DELETE /v2/{name}/blobs/uploads/{uuid}` — cancel upload
   - `HEAD|GET /v2/{name}/manifests/{reference}` — manifest get
   - `PUT /v2/{name}/manifests/{reference}` — manifest put
   - `DELETE /v2/{name}/manifests/{reference}` — manifest delete
   - `GET /v2/{name}/tags/list` — list tags
   - `GET /v2/_catalog` — catalog

3. **HMAC state token**：精确移植 `handlers/hmac.go`。`blobUploadState { name, uuid, offset, started_at }` 序列化为 JSON，HMAC-SHA256 + base64url 签名，通过 `_state` query param 传递。用 `hmac 0.12.x` + `sha2 0.10.x` + `base64 0.22.x` 实现。

4. **Streaming PATCH body**：axum `Body` 作为 `AsyncRead` 流式写入 `FileWriter`（`tokio::io::copy`），不在内存中缓冲整个 blob。

5. **Range 请求处理**：用 `http-range 0.1.x` 解析 `Range` header，对 `BlobProvider::open()` 返回的 `AsyncSeekRead` 执行 seek，构造 `206 Partial Content`。不用 `tower-http::ServeDir`（面向静态文件，不适合抽象存储）。

6. **name 参数校验**：OCI spec 正则校验在 axum extractor 中完成，失败返回 `400 NAME_INVALID`。

7. **Manifest content negotiation**：按 `Accept` header 优先级返回格式，无匹配则返回存储的原始格式。

**完成标准：**
```bash
# 启动（filesystem driver，无 auth）
cargo run -p registry-bin -- --config test-config-noauth.yaml

# 运行 OCI Distribution Spec conformance tests
# （clone https://github.com/opencontainers/distribution-spec 编译 conformance.test）
OCI_ROOT_URL=http://localhost:5000 \
OCI_NAMESPACE=test/myrepo \
OCI_TEST_PULL=1 OCI_TEST_PUSH=1 \
OCI_TEST_CONTENT_DISCOVERY=1 \
OCI_TEST_CONTENT_MANAGEMENT=1 \
./conformance.test -test.v -test.run "TestBasic|TestPush|TestPull|TestContentDiscovery"

# 所有以上类别的测试必须 PASS，零失败
```

---

### Phase 4：Auth 与中间件

**目标：** 实现 `AccessController` trait + token JWT auth + htpasswd auth，使 `docker login / push / pull` 端到端可用。

**包含模块：** `registry-auth`（token/、htpasswd/、silly/），`registry-http/src/middleware/auth.rs`

**关键决策点：**

1. **AccessController trait**：
   ```rust
   #[async_trait]
   pub trait AccessController: Send + Sync {
       async fn authorized(
           &self,
           headers: &HeaderMap,
           access: &[Access],
       ) -> Result<Grant, AuthError>;
   }

   pub enum AuthError {
       Challenge(AuthChallenge),  // → 401 + WWW-Authenticate
       Unauthorized(String),
       Internal(anyhow::Error),
   }
   ```
   `AuthChallenge` 携带 realm/service/scope，由 HTTP 中间件写入 `WWW-Authenticate` 响应头。解耦了 Go 中 `Challenge` interface 同时是 error 又写 HTTP header 的问题。

2. **Token auth 流程**（移植 `registry/auth/token/accesscontroller.go`）：
   - 从 `Authorization: Bearer <token>` 提取 JWT
   - 用 `jsonwebtoken 9.x` 验证 RS256/ES256 签名（从 PEM 或 JWKS 加载公钥）
   - 验证 issuer/audience/expiry
   - 对比 token `access` claims 与请求所需 access
   - 失败返回 `AuthError::Challenge`（携带 `WWW-Authenticate: Bearer realm=...,service=...,scope=...`）

3. **htpasswd**：仅支持 bcrypt（`$2y$`，使用 `bcrypt 0.15.x`）。SHA1/MD5 格式写警告日志并拒绝（不安全）。文件 mtime 变化时动态重载。

4. **auth 中间件位置**：中间件只处理"无 token"场景（返回 challenge）。细粒度 scope 检查在各 handler 内调用 `access_controller.authorized()`，与 Go 的 `app.authorized()` 位置一致。

5. **无 auth 模式**：config 无 `auth:` 节时注入 `NullAccessController`，直接返回全权 `Grant`。

**完成标准：**
```bash
# 配置 token auth（测试 RSA key pair）
docker login localhost:5000 -u testuser -p testpassword
# → Login Succeeded

docker pull ubuntu:22.04
docker tag ubuntu:22.04 localhost:5000/myorg/ubuntu:22.04
docker push localhost:5000/myorg/ubuntu:22.04
docker pull localhost:5000/myorg/ubuntu:22.04
# → 全部成功

# 未认证请求返回 401
curl -sv http://localhost:5000/v2/myorg/ubuntu/tags/list 2>&1 | grep -E "401|WWW-Authenticate"
# → HTTP/1.1 401 Unauthorized
# → WWW-Authenticate: Bearer realm=...
```

---

### Phase 5：配置、通知与运维能力

**目标：** 实现完整 YAML 配置加载、webhook 通知、健康检查、Prometheus metrics、结构化日志。达到可生产部署状态。

**包含模块：** `registry-config`、`registry-notifications`、`registry-health`，以及 `registry-http` 中的 health/metrics handler 和 tracing 集成。

**关键决策点：**

1. **配置加载**：`figment 0.10.x`（YAML 文件 + 环境变量覆盖）。`Configuration` struct 精确对应 Go 的 `configuration/configuration.go` 顶层字段：`version`, `log`, `storage`, `auth`, `http`, `notifications`, `health`, `redis`, `catalog`, `proxy`, `validation`。storage driver 参数用 `HashMap<String, serde_json::Value>` 对应 Go 的 `Parameters`。支持 `${ENV_VAR}` 替换（反序列化前预处理）。
   - 选 figment 而非 `config` crate 原因：figment 反序列化错误信息更具体（精确到字段名），对配置调试体验更好。

2. **Notifications**：
   ```rust
   let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<NotificationEnvelope>();
   tokio::spawn(async move {
       while let Some(env) = rx.recv().await {
           for sink in &sinks { sink.send(env.clone()).await; }
       }
   });
   ```
   HTTP endpoint sink 用 `reqwest 0.12.x` POST，指数退避重试（`tokio::time::sleep` 实现，不引入额外 backoff crate）。Event 格式与 Go 完全兼容（`application/vnd.docker.distribution.events.v2+json`）。

3. **Prometheus metrics**（`prometheus 0.13.x`）：
   - `registry_http_requests_total{method, route, status}` counter
   - `registry_storage_action_duration_seconds{driver, action}` histogram
   - `registry_blob_size_bytes` histogram
   - 暴露 `GET /metrics`（Prometheus text format）

4. **健康检查**：`GET /healthz` 返回 JSON。Filesystem driver 健康检查：验证 root 目录可写；S3 driver：执行一次 list 操作。

5. **结构化日志**：`tracing 0.1.x` + `tracing-subscriber` JSON format。每个请求附加 `request_id`（UUID）、`repository`、`method`、`path` 字段，对应 Go 的 `dcontext.GetLogger(ctx)`。

**完成标准：**
```bash
# 配置校验
cargo run -p registry-bin -- --config /path/to/config.yaml --check-config
# → Config OK（或具体错误位置）

# 健康检查
curl http://localhost:5000/healthz
# → {"status":"ok"}

# Metrics 端点
curl http://localhost:5000/metrics | grep registry_http_requests_total
# → 有输出

# Notifications（配置 webhook endpoint）
docker push localhost:5000/test/image:latest
# → webhook 在 5 秒内收到 action="push" Event，包含 target.repository/digest/tag

# Phase 3/4 regression
OCI_ROOT_URL=http://localhost:5000 ./conformance.test -test.v  # 仍全绿
docker push/pull 仍然正常工作
```

---

## 关键 Rust 设计决策

| Go 惯用法 | Rust 方案 | 理由 |
|----------|-----------|------|
| `Manifest interface{}` | `ManifestBody` enum（4 variant） | 类型集合封闭，enum 消除 downcast，无堆分配；可加 `Custom` variant 扩展 |
| `context.Context` 键值存储 | 显式 `RequestContext` struct | Go `ctx.Value(key)` 无编译期类型保证；Rust 强类型结构体更安全 |
| `context.Context` 取消传播 | `tokio_util::sync::CancellationToken`（`tokio-util 0.7.x`） | tokio future 本身支持取消；`CancellationToken` 用于显式跨任务取消 |
| `errgroup` goroutine 并发 | `tokio::task::JoinSet` / `futures::future::try_join_all` | tokio 是 Rust 生态事实标准；async-std 主要 I/O 库（reqwest/sqlx/object_store）均以 tokio 为首要目标 |
| `init()` 全局注册 | `RegistryBuilder::register_storage_driver("fs", Factory)` 显式注册 | Rust 无 `init()`；`inventory` crate 可模拟但引入 proc-macro 魔法；显式注册更易审查和测试 |
| `BlobStore.ServeBlob(http.ResponseWriter)` | 移到 HTTP handler，调用 `BlobProvider::open()` | 解除存储层对 HTTP 的耦合；存储层可被非 HTTP 场景（CLI、备份工具）复用 |
| `http.ServeContent`（Range 处理） | `http-range 0.1.x` 解析 + `AsyncSeekRead` + 手动 206 | `tower-http ServeDir` 面向磁盘文件不适合；自行实现约 40 行，可控 |
| `Challenge` interface（error + 写 header） | `AuthError::Challenge(AuthChallenge)` + HTTP 中间件处理 | 职责分离；存储/auth 层不应知道 HTTP response 格式 |
| `ManifestServiceOption` variadic | `GetManifestOpts` / `PutManifestOpts` struct + `Default` | Rust 无 variadic；struct 字面量 + Default 更清晰，IDE 补全更好 |
| SHA256 `BinaryMarshaler`（可恢复 digest） | Phase 1: re-hash on resume（O(N) 但正确）；后续评估自定义实现 | `sha2` 不暴露内部状态；re-hash 保证正确性且不阻塞 Phase 1 交付 |
| `Storage` map 参数（`map[string]interface{}`） | `HashMap<String, serde_json::Value>` | 类型等价；figment 反序列化支持此模式 |

---

## 风险与注意事项

| 风险 | 严重程度 | 应对策略 |
|------|---------|---------|
| **SHA256 中间状态不可序列化**（`sha2` crate 内部状态私有，影响大文件断点续传） | 高 | Phase 1 用 re-hash-on-resume；Phase 3 后评估是否引入自定义 SHA256 实现或将状态存 Redis |
| **OCI conformance test header 细节**（`Docker-Content-Digest` 位置、`206` 格式、`Link` header 分页） | 中 | Phase 3 期间逐个测试用例推进（`-test.run TestBasic` 等），不要等最后统一运行 |
| **paths.go 精确对应**（路径偏差将无法读取现有 Go registry 数据） | 高 | 对每个 pathSpec 写双重验证单元测试（Rust 输出 vs Go 输出字符串比对），Phase 1 必须通过 |
| **token auth JWT 细节兼容性**（Docker token service 的 `access` claim 格式、算法协商） | 中 | 从 `registry/auth/token/token_test.go` 提取测试向量，在 Rust 单元测试中用相同数据验证 |
| **axum 中间件无法访问 path 参数**（auth 中间件需要 `{name}` 计算 scope，但 tower Layer 在 router 外层） | 低 | scope 检查移入各 handler 函数（与 Go 的 `app.authorized()` 位置一致），中间件只负责无 token 时返回 challenge |
| **Redis client 选型**（fred 8.x API 稳定性存疑） | 低 | 先用 `redis 0.27.x`（更保守）；Redis 仅用于 descriptor cache，不影响正确性，客户端可随时切换 |
| **Windows 不支持**（`tokio::fs::rename` 在 Windows 若目标存在会失败） | 低 | 文档声明 filesystem driver 仅支持 POSIX；Windows 用 in-memory driver 做测试 |

---

## 验证策略（端到端）

```
Phase 0: cargo test --workspace（纯单元测试）
Phase 1: cargo test --workspace（含 driver compliance + 路径布局精确匹配）
Phase 2: cargo test -p registry-integration-tests（in-process 存储层 API 测试）
Phase 3: OCI Distribution Spec conformance test（Basic/Push/Pull/ContentDiscovery 全绿）
Phase 4: docker login + docker push + docker pull 端到端（带 token auth）
Phase 5: Phase 3/4 回归 + health/metrics/webhook 冒烟测试
```

**关键外部依赖（需提前准备）：**
- OCI conformance test binary：clone `github.com/opencontainers/distribution-spec`，编译 `conformance.test`
- 测试用 RSA key pair（Phase 4）：`openssl genrsa -out private.pem 2048 && openssl rsa -in private.pem -pubout -out public.pem`
- Webhook 接收端（Phase 5）：本地 `nc -l 8080` 或 webhook.site