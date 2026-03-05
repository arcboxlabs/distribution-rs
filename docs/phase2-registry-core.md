# Phase 2：Registry 核心逻辑

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**目标：** 实现 `ManifestService`、`BlobStore`（含 `LinkedBlobStore`）、`TagService`，构建完整 namespace/repository 层级，支持通过存储层 API 直接完成 push/pull（不经过 HTTP）。

**包含模块：** `registry-storage`（registry.rs、blob_access_controller.rs、cache/memory.rs、gc.rs）、`registry-integration-tests`（新建）

## 核心接口（Go 对应）

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

## 关键决策点

1. **ManifestHandler dispatch**：Go 的 `manifestStore` 持有四个 handler（按 media type 分发）。Rust 版本用 `HashMap<&'static str, Box<dyn ManifestHandler>>`，PUT 时按 `Content-Type` 选 handler，GET 时按存储 JSON 自动检测类型。

2. **LinkedBlobStore 仓库隔离**：`BlobProvider::get()` 前先检查 `_layers/{repo}/sha256/{hash}/link` 存在，不存在返回 `BlobUnknown`。这是安全边界，不可省略。

3. **跨仓库 blob mounting**：`BlobCreateOption::MountFrom { source_repo, digest }` — 检查源仓库有 link，若有则直接在目标仓库创建 link，返回 `Mounted` 信号。

4. **Descriptor 缓存**：接入 in-memory LRU 缓存（`lru 0.12.x`）包装 `BlobDescriptorService`，热路径 `stat()` 不走 storage driver。

5. **Arc<dyn StorageDriver> 而非泛型**：registry/repository 结构体始终用 `Arc<dyn StorageDriver>`，避免泛型爆炸。

## 依赖 crate

| crate | 版本 | 用途 |
|-------|------|------|
| `lru` | 0.12.x | BlobDescriptor in-memory 缓存 |
| `uuid` | 1.x | 生成上传 UUID |

## 完成标准

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
