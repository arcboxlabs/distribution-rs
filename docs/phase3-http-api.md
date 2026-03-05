# Phase 3：HTTP API 层

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**目标：** 实现 axum HTTP 服务器，通过 OCI Distribution Spec conformance test 的 Basic / Push / Pull / Content Discovery 四类测试。

**包含模块：** `registry-http`、`bin/registry`（无 auth 的简单配置）

## 目录结构

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

## AppState 定义

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

## 完整路由表

对应 Go 的 `v2.RouteNameXxx` 常量：

| 路由 | 方法 | 说明 |
|------|------|------|
| `/v2/` | GET | version check |
| `/v2/{name}/blobs/{digest}` | HEAD, GET | blob get |
| `/v2/{name}/blobs/{digest}` | DELETE | blob delete |
| `/v2/{name}/blobs/uploads/` | POST | start upload |
| `/v2/{name}/blobs/uploads/{uuid}` | GET, HEAD | upload status |
| `/v2/{name}/blobs/uploads/{uuid}` | PATCH | patch data |
| `/v2/{name}/blobs/uploads/{uuid}` | PUT | complete upload |
| `/v2/{name}/blobs/uploads/{uuid}` | DELETE | cancel upload |
| `/v2/{name}/manifests/{reference}` | HEAD, GET | manifest get |
| `/v2/{name}/manifests/{reference}` | PUT | manifest put |
| `/v2/{name}/manifests/{reference}` | DELETE | manifest delete |
| `/v2/{name}/tags/list` | GET | list tags |
| `/v2/_catalog` | GET | catalog |

## 关键决策点

1. **AppState**：见上方定义。所有字段 `Arc` 包裹，`AppState: Clone + Send + Sync`，axum 通过 `State<AppState>` 提取。

2. **HMAC state token**：精确移植 `handlers/hmac.go`。`blobUploadState { name, uuid, offset, started_at }` 序列化为 JSON，HMAC-SHA256 + base64url 签名，通过 `_state` query param 传递。用 `hmac 0.12.x` + `sha2 0.10.x` + `base64 0.22.x` 实现。

3. **Streaming PATCH body**：axum `Body` 作为 `AsyncRead` 流式写入 `FileWriter`（`tokio::io::copy`），不在内存中缓冲整个 blob。

4. **Range 请求处理**：用 `http-range 0.1.x` 解析 `Range` header，对 `BlobProvider::open()` 返回的 `AsyncSeekRead` 执行 seek，构造 `206 Partial Content`。不用 `tower-http::ServeDir`（面向静态文件，不适合抽象存储）。

5. **name 参数校验**：OCI spec 正则校验在 axum extractor 中完成，失败返回 `400 NAME_INVALID`。

6. **Manifest content negotiation**：按 `Accept` header 优先级返回格式，无匹配则返回存储的原始格式。

## 依赖 crate

| crate | 版本 | 用途 |
|-------|------|------|
| `axum` | 0.8.x | HTTP 框架 |
| `tower` | 0.5.x | 中间件 |
| `tower-http` | 0.6.x | HTTP 中间件（tracing、compression） |
| `hmac` | 0.12.x | state token 签名 |
| `sha2` | 0.10.x | HMAC-SHA256 |
| `base64` | 0.22.x | base64url 编解码 |
| `http-range` | 0.1.x | Range header 解析 |
| `clap` | 4.x | CLI 参数（bin/registry） |

## 完成标准

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

## 注意事项

- OCI conformance test header 细节（`Docker-Content-Digest` 位置、`206` 格式、`Link` header 分页）：Phase 3 期间逐个测试用例推进（`-test.run TestBasic` 等），不要等最后统一运行
