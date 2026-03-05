# Phase 0：基础设施

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**目标：** 建立 Rust workspace，实现所有核心类型（Digest、Reference、OCI 错误码、Descriptor、ManifestBody），无 I/O、无网络，纯数据层。后续所有 crate 依赖本 phase 输出。

**包含模块：** `registry-core`、`registry-reference`

## Workspace 目录结构

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

## 关键决策点

1. **Digest 类型**：自行实现（`struct Digest { algorithm: DigestAlgorithm, encoded: String }`），不依赖第三方 digest crate（`opencontainers-digest` 维护状态不佳）。用 `sha2 0.10.x` + `hex 0.4.x` 实现 `Digester`。
2. **ManifestBody 为 enum**：Schema2 / OCI Manifest / DockerManifestList / OCI Index 四个 variant。类型集合封闭，编译期穷举，避免 `dyn Manifest` 的堆分配和 downcast。
3. **Reference parsing**：直接从 Go 的 `distribution/reference` 移植正则 grammar，使用 `regex 1.x`。
4. **OCI 错误码**：`enum OciErrorCode`（BLOB_UNKNOWN, DIGEST_INVALID 等）+ `struct OciErrors { errors: Vec<OciError> }` 可序列化为 OCI 规范 JSON。

## 依赖 crate

| crate | 版本 | 用途 |
|-------|------|------|
| `serde` | 1.x | 序列化/反序列化 |
| `thiserror` | 2.x | 错误类型定义 |
| `sha2` | 0.10.x | SHA256 digest 计算 |
| `bytes` | 1.x | 字节操作 |
| `hex` | 0.4.x | digest 编码 |
| `regex` | 1.x | reference 正则解析 |

## 完成标准

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
