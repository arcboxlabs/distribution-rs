# Phase 1：存储层

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**目标：** 实现 `StorageDriver` trait 及文件系统 driver，实现 blob 上传状态机（POST/PATCH/PUT 三步流程），建立路径布局规范。通过 driver compliance test suite。

**包含模块：** `registry-storage-driver`、`registry-storage-driver-fs`、`registry-storage`（paths、blob_writer、blob_store、linked_blob_store、manifest_store、tag_store）

## 目录结构

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

## StorageDriver trait（Go 接口对应）

**Go 原始接口** (`registry/storage/driver/storagedriver.go`)：
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

## 关键决策点

1. **StorageDriver trait 用 `async-trait` 宏**（0.1.83+）：使 `dyn StorageDriver + Send + Sync` 对象安全。AFIT（Rust 1.75+）在 `dyn` 下仍有对象安全限制，`async-trait` 绕过此问题。
2. **FileWriter 原子写入**：写入 UUID 命名临时文件，`commit()` 调用 `tokio::fs::rename`（POSIX 原子），`cancel()` 删除临时文件。
3. **walk() 返回 Stream**：替换 Go 的回调 `WalkFn`，返回 `BoxStream<'_, Result<FileInfo, StorageError>>`（`futures 0.3.x`），更符合 Rust 组合风格。
4. **BlobServer 从存储层移除**：Go `BlobStore` 包含 `ServeBlob(w http.ResponseWriter, ...)` 是反模式。Rust 版本 blob 服务逻辑在 HTTP handler 层（调用 `BlobProvider::open()` + 手动 Range 处理）。
5. **可恢复 digest 状态**：Go 用 `encoding.BinaryMarshaler` 将 SHA256 hasher 内部状态持久化到 `_uploads/{uuid}/hashstates/sha256/{offset}`。`sha2 0.10.x` 不暴露内部状态序列化 API。**Phase 1 决定：实现非可恢复模式**（对应 Go 的 `blobwriter_nonresumable.go`），PATCH 续传时重新读已写数据重算 SHA256。标注 `// TODO(resumable-digest)`，Phase 3 后评估。
6. **paths.rs 精确对应 paths.go**：路径布局必须与 Go 实现 byte-for-byte 兼容（保证可读取现有 Go registry 数据）。每个 pathSpec variant 写单元测试验证路径字符串完全匹配。

## 依赖 crate

| crate | 版本 | 用途 |
|-------|------|------|
| `async-trait` | 0.1.x | StorageDriver trait 对象安全 |
| `tokio` | 1.x | 异步运行时、tokio::fs |
| `futures` | 0.3.x | BoxStream for walk() |
| `tempfile` | 3.x | 临时文件原子写入 |
| `lru` | 0.12.x | BlobDescriptor 缓存 |
| `uuid` | 1.x | upload UUID 生成 |
| `hmac` | 0.12.x | HMAC state token |

## 关键 Go 参考文件

- `registry/storage/driver/storagedriver.go` — trait 定义来源
- `registry/storage/paths.go` — 路径布局规范（**必须精确对应**）
- `registry/storage/blobwriter.go` — `Commit`/`Cancel`/`validateBlob`/`moveBlob` 逻辑
- `registry/storage/driver/filesystem/driver.go` — 文件系统实现参考

## 完成标准

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
